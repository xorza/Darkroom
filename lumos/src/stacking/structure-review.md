# Stacking structure audit — remaining work

Scope: production Rust under `lumos/src/stacking`, reconciled after the cancellation, public API,
canonical product, typed-validation, weight-normalization, coherent drizzle-frame, and fallible
star-detection configuration batches. Tests and benchmarks are excluded from the assessment.

## Completed since the original audit

- Defect-map cancellation is fallible; partial calibration state no longer escapes.
- The crate root exports the full public stacking configuration graph.
- Combine, drizzle, and the pipeline share one canonical `StackProduct`.
- Stack and drizzle configuration failures, manual-weight counts, coverage dimensions, and drizzle
  accumulator construction return typed errors instead of panicking.
- Manual weights remain scale-invariant even when their finite positive sum is below
  `f32::EPSILON`.
- Drizzle paths and images use coherent `DrizzleFrame` records; public accumulator insertion
  validates dimensions and weights through typed errors before mutating state.
- Star-detection configuration and its nested public values return `StarDetectionConfigError`;
  detector construction validates once, and alignment pipelines propagate that error.

## Architectural issues

### Pipeline storage still depends on combine internals

The streaming pipeline imports `Plane`, `WeightedFrame`, spill/remove helpers, cache sizing, and
`stack_weighted_frames`, then stores `Plane` directly in `DetectedFrame`. This makes orchestration
construct the combine backend's private representation and prevents either layer from changing
independently.

References: `pipeline/mod.rs:20-28`, `pipeline/mod.rs:505-507`, `pipeline/mod.rs:560-678`.

Extract `stacking::frame_store` for memory planning, RAM/mmap planes, spill cleanup, and stored
frames. Pipeline should produce stored frames through that API; combine should consume them without
pipeline naming cache implementation types.

### Calibration masters expose derived state that callers can invalidate

`CalibrationMasters` publicly exposes four optional images and the defect map. A caller can replace
the dark or flat while retaining a defect map derived from the previous images. The three role
containers also repeat the same four roles with plural/master naming drift.

References: `calibration_masters/mod.rs:42-88`, `calibration_masters/mod.rs:161-216`.

Keep the master images and defect map private behind a validated `CalibrationMasters` boundary.
Represent the four image roles once and use that component for pre-stacked inputs and stored
masters; expose only operations and non-trivial derived views.

### Stage configuration is still flat and widely coupled

Star detection passes its entire 40-plus-field `Config` into background, detection, FWHM,
measurement, and filtering stages. Registration similarly copies fields from one flat config into
`TriangleParams`, `RansacParams`, `SipConfig`, and `WarpParams`.

References: `star_detection/config.rs:177-303`, `star_detection/detector/stages/*.rs`,
`registration/config.rs:110-168`, `registration/mod.rs:150-154`, `registration/mod.rs:380-434`.

Compose stage-owned configuration types in the public high-level configs. Each stage should accept
only its own configuration, eliminating translation blocks and unrelated coupling.

## Simplifications

### Split large files only after responsibilities move

`combine/cache.rs` is 2,154 lines and owns storage, tier selection, disk persistence, and chunk
execution. `drizzle/mod.rs` is 943 lines and owns config, accumulation, five kernels, geometry, and
entry points. `pipeline/mod.rs` is 919 lines and owns both RAM and streaming orchestration.

After extracting `frame_store`, split combine cache into load/store/engine, drizzle into
config/accumulator/geometry/stack, and pipeline into config/result/align/streaming. Splitting first
would only redistribute the current coupling.

### Name registration relationships instead of using tuples

`RegistrationResult::matched_stars` and `recover_matches` use `(usize, usize)` for reference and
target indices, and recovery returns a tuple containing the transform and matches.

References: `registration/result.rs:119-145`, `registration/mod.rs:476-490`.

Introduce `StarMatch { reference, target }` and `RecoveredMatches { transform, matches }`.

### Remove intermediate data copies in star detection

`StarMetrics` duplicates most fields of `Star`, while `QualityFilterStats::apply_to` copies seven
counters into `Diagnostics`.

References: `star_detection/centroid/mod.rs:712-895`,
`star_detection/detector/stages/filter.rs:24-45`, `star_detection/detector/mod.rs:35-68`.

Construct the canonical star value directly or embed a shared metrics component; likewise embed one
quality diagnostics component rather than copying counters field by field.

### Delete TPS unless it has an immediate integration owner

The entire thin-plate-spline implementation is blanket-allowed as dead code and has no production
caller. Carrying a second distortion model without an integration path adds a large dormant API and
maintenance surface.

References: `registration/distortion/tps/mod.rs:1-5`, `registration/distortion/mod.rs:30`.

Delete it now and restore it from history if a concrete post-RANSAC TPS mode is scheduled.

## Smaller improvements

- Move `combine::progress` to `stacking::progress`; drizzle, calibration, and pipeline already use it.
- Move public `warp` and `WarpResult` from `registration/mod.rs` to `registration/resample.rs`.
- Narrow internal `pub` items in cache sizing, triangle matching, RANSAC, interpolation, and progress.
- Refresh `lumos/CLAUDE.md` and stale stage READMEs after the structural moves; cancellation and CFA
  cosmic-ray descriptions already lag the implementation.

## Open questions

- Does TPS have a near-term owner and caller? If not, deletion is the simpler state.
- Do callers need to mutate individual calibration masters after construction? If not, the public
  fields should close immediately.

## Prioritized shortlist

1. Encapsulate `CalibrationMasters` so its defect map cannot drift from its source images.
2. Extract `frame_store` and shared progress, then split cache/drizzle/pipeline by ownership.
3. Compose stage configs; add named registration results; consolidate star diagnostics; delete TPS
   if it has no owner.
