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
- `CalibrationMasters` keeps source images and its derived defect map private; construction rebuilds
  defects, while component presence and counts escape only through read-only derived views.
- `stacking::frame_store` owns memory planning, RAM/mmap planes, spill cleanup, and stored-frame
  records. Pipeline uses coherent `DetectedFrame<I>` records in both tiers and produces stored light
  frames without importing combine cache internals.
- Combine cache is split by ownership: `cache/loader/mod.rs` owns tier selection and persistent
  sidecars, `cache/mod.rs` owns chunk execution and product quality planes, and each has colocated
  tests.
- Pipeline is split by ownership: `config.rs` and `result.rs` own the public data model, `align.rs`
  owns calibrated-image alignment, and `streaming.rs` owns RAW calibration plus memory-tier
  orchestration.
- `stacking::progress` owns progress reporting shared by combine, drizzle, calibration, and pipeline;
  its callback dispatcher is crate-private while the crate root exports the public data types.
- Drizzle is split by ownership: `config.rs` owns kernel/configuration types, `accumulator.rs` owns
  coherent frames and pixel accumulation, `geometry.rs` owns pure overlap/Jacobian math, and
  `stack.rs` owns loading and orchestration.
- Registration uses `StarMatch` for confirmed reference/target correspondences and
  `RecoveredMatches` for the recovery stage's transform-plus-matches result.

## Architectural issues

### Calibration roles remain duplicated across containers

`CalibrationFrames`, `CalibrationImages`, and the private `CalibrationMasters` storage repeat the
same four roles with plural/master naming drift. `CalibrationComponent` provides canonical query
names but does not yet remove the repeated storage declarations.

References: `calibration_masters/mod.rs:42-121`, `calibration_masters/mod.rs:236-280`.

Represent the four image roles once and reuse that component for raw paths, pre-stacked inputs, and
stored masters without weakening the private defect-map invariant.

### Stage configuration is still flat and widely coupled

Star detection passes its entire 40-plus-field `Config` into background, detection, FWHM,
measurement, and filtering stages. Registration similarly copies fields from one flat config into
`TriangleParams`, `RansacParams`, `SipConfig`, and `WarpParams`.

References: `star_detection/config.rs:177-303`, `star_detection/detector/stages/*.rs`,
`registration/config.rs:110-168`, `registration/mod.rs:150-154`, `registration/mod.rs:380-434`.

Compose stage-owned configuration types in the public high-level configs. Each stage should accept
only its own configuration, eliminating translation blocks and unrelated coupling.

## Simplifications

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

- Move public `warp` and `WarpResult` from `registration/mod.rs` to `registration/resample.rs`.
- Narrow internal `pub` items in cache sizing, triangle matching, RANSAC, and interpolation.
- Refresh `lumos/CLAUDE.md` and stale stage READMEs after the structural moves; cancellation and CFA
  cosmic-ray descriptions already lag the implementation.

## Open questions

- Does TPS have a near-term owner and caller? If not, deletion is the simpler state.

## Prioritized shortlist

1. Compose stage configs.
2. Consolidate calibration roles; consolidate star diagnostics; delete TPS
   if it has no owner.
