# lumos: one layout for files, types and entry points

Scope: the `lumos` crate's module tree, its file and type names, and its public entry points, under
the six rules below; `lens` and `darkroom` follow each public change. No behaviour changes: every
item is a move, a rename, a split, a merge of types that count the same thing, or a change of where
an entry point lives. A doc that names a moved path or a renamed type is updated with it; the doc
gate catches a broken intra-doc link.

## Rules

1. A module is a domain (what the data is: `image/`, `frame/`), a stage (what the pipeline does:
   `calibration/`, `star_detection/`, `registration/`, `combine/`, `drizzle/`, `pipeline/`,
   `image_ops/`, `ingest/`) or support (how: `math/`, `simd/`, `platform/`, `memory/`,
   `concurrency/`, `progress/`, `io/`). `io/` reads and writes files and holds no domain type.
2. One home per concept.
3. Every stage has one shape: `mod.rs` with the docs, `<name>_config.rs` for a config,
   `error.rs` for its errors and nothing else, `<name>_result.rs` for a result. A major struct
   lives in the file of its own name.
4. An operation is a method on its config or its owner, as `Denoise::apply`,
   `CalibrationMasters::calibrate` and `StarDetector::detect` are. No public free functions.
5. A public type's name is unique without its path; the facade is flat.
6. One input has one name in every stage: an entry over files ends in `_paths`, one over images
   in `_images`.

## Items

Done in this order; each is deleted when it is committed.

1. **`image/`: the domain types out of `io/image/`.** New top-level `image/` holds what
   `io/image/` holds except the file formats: `linear_pixels`, `image_dimensions`,
   `image_metadata`, `image_provenance`, `calibration_state`, `sample_domain`, `flat_gain/`,
   `preview_image`, `pixel_flags`, `cfa/`. `linear.rs` becomes `linear_image.rs`. Splits:
   `QualityFlags` from `pixel_flags.rs` into `quality_flags.rs`; `CfaType` and `CfaImage` from
   `cfa/mod.rs` into `cfa_type.rs` and `cfa_image.rs`, each satellite (`CfaFrameInfo`) with the
   type it serves. `InvalidFlatGain` and `XTransPatternError` move to a new `image/error.rs`.
   `io/` keeps the formats, one level up: `io/fits/` (from `io/image/fits/`, with
   `sample_type_record.rs`), `io/raw/`, `io/standard.rs`, `io/input_format.rs`,
   `io/load_context.rs`, `io/error.rs` (`ImageError`), and `PREVIEW_IMAGE_EXTENSIONS` in
   `io/mod.rs`. `unverified_conditions.rs` waits for item 3 where it is.

2. **One home each for demosaic, the mosaic noise and `Cancelled`.** `io/raw/demosaic/` moves to
   `image/cfa/demosaic/`: `CfaImage` demosaics a mosaic from any source, not only a raw file.
   `difference_noise.rs` and `mosaic_noise.rs` move to `math/noise/` beside `mrs_noise.rs`.
   `Cancelled` moves from `io/cancelled.rs` into the crate's `error.rs`.

3. **`frame/`: what is known about a frame, out of `frame_store/`.** New top-level `frame/` holds
   `frame_facts`, `frame_stats/`, `frame_quality`, `frame_peek`, `stackable_image`,
   `capture_conditions`, and `unverified_conditions` from `io/image/`. `frame_store/` keeps where a
   run's frames live: `stored_*`, `plane_store`, `run_scratch`, `disk_root`, `tier`,
   `stratified_samples`, `error`. The frames kept across runs go to `frame_store/decode_cache/`:
   `DecodeCache` in its `mod.rs`, with `cache_key`, `cached_source` and `frame_spill`.

4. **`calibration/`.** `calibration_masters/` becomes `calibration/`; `CalibrationMasters` moves
   from its `mod.rs` to `calibration_masters.rs`. `cosmic_ray/config.rs` becomes
   `cosmic_ray/cosmic_ray_config.rs`.

5. **Registration in the stage shape.** `RegistrationError` moves from `result/mod.rs` to a new
   `registration/error.rs`; `result/` becomes `registration_result/`. `WarpParams` and
   `InterpolationMethod` move from `registration_config/` to `resample/warp_params.rs`.
   `ransac/config.rs` becomes `ransac/ransac_config.rs`; `TriangleConfig` moves from
   `triangle/mod.rs` to `triangle/triangle_config.rs`; `WarpTransform` from `transform/mod.rs` to
   `transform/warp_transform.rs`; `spatial/` becomes `kd_tree/`.

6. **Star detection in the stage shape.** `star_detection/config/mod.rs`'s `Config` moves to
   `config/star_detection_config.rs`; `StarDetector` from `detector/mod.rs` to
   `detector/star_detector.rs`, and `DetectionResult` with its diagnostics to
   `detector/detection_result.rs`. `centroid/lm_optimizer.rs` becomes
   `centroid/profile_problem.rs`. `background_mesh/mod.rs`'s `TileGrid` moves to
   `background_mesh/tile_grid.rs`.

7. **Combine, drizzle, pipeline and the product in the stage shape.** `combine/config/` becomes
   `combine/stack_config/`; `combine/cache/` becomes `combine/frame_cache/` (`FrameCache`), so
   "cache" names the decode cache and the master cache only. `drizzle/config.rs` becomes
   `drizzle/drizzle_config.rs`; the public `DrizzleFrame` moves out of `accumulator/` to
   `drizzle/drizzle_frame.rs`. `pipeline/config.rs` becomes `pipeline/align_stack_config.rs`,
   `pipeline/result.rs` becomes `pipeline/align_stack_result.rs`. `run_report.rs` moves into
   `stack_product/`.

8. **Image ops named after their ops.** `stretching/` becomes `stretch/`; `color_calibration/`
   splits into `neutralize_background.rs` and `scnr.rs`; `background_extraction/` becomes
   `extract_background/`; `ml/star_removal.rs` becomes `ml/remove_stars.rs`, `ml/denoise.rs`
   becomes `ml/ml_denoise.rs`; `MlError` moves from `ml/backend/` to `image_ops/error.rs`, gated
   on `ml` as it is now.

9. **One flag counter.** `stack_product/run_report.rs`'s `FlagCounts`, `LocalFlagCounts` and
   `AtomicFlagCounts`, and `image/pixel_flags.rs`'s `FlagCounts`, count samples by flag. Merge
   them into one `FlagCounts` indexed by `QualityFlags`, and its atomic form for the workers. The
   public accessors the run report gives keep the counts it reports now.

10. **Unique names and a flat facade.** Renames: `Config` → `StarDetectionConfig`;
    `Diagnostics` → `DetectionDiagnostics`; the detect stage's `DetectionConfig` →
    `ExtractionConfig`, so it does not read as the whole detection's; the quality filter's private
    `Rejection` → `FilterRejection`; `centroid/local_background.rs`'s `LocalBackground` →
    `StampBackground` (file renamed with it). `lib.rs` drops `pub mod detection` and exports its
    types at the root. `lens` follows.

11. **Entry points as methods, one name per input.** `stack` and `stack_images` become
    `StackConfig::stack_paths` and `StackConfig::stack_images`; `drizzle_stack`,
    `drizzle_images`, `drizzle_cfa_stack`, `drizzle_cfa_images` become `DrizzleConfig::{
    drizzle_paths, drizzle_images, drizzle_cfa_paths, drizzle_cfa_images}`, and `DrizzleConfig`
    holds `stack: StackConfig` as `AlignStackConfig` does, in place of a second parameter;
    `align_and_stack` and `align_and_stack_paths` become `AlignStackConfig::align_stack_images`
    and `align_stack_paths`, and `calibrate_align_stack` becomes
    `AlignStackConfig::calibrate_align_stack_paths`; `register` becomes
    `RegistrationConfig::register`; `warp` becomes `WarpTransform::warp`. `IngestConfig` leaves
    `AlignStackConfig` and `DrizzleConfig` and is a parameter of every entry that reads or spills
    frames, as `stack` takes it now; `stack_images` keeps every frame in memory and takes none.
    Each method lives in its type's own file, as every `impl` does, and calls the stage's code
    where it is. `lens`, `darkroom`, the benches and the docs follow.
