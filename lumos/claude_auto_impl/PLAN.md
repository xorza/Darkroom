# Lumos API Improvement Plan

Improve lumos public API for consistency, simplicity, and convenience.

## Phase 1: Replace raw pixels/width/height with AstroImage

Many functions take `(pixels: &[f32], width: usize, height: usize)` instead of `&AstroImage`. Add convenient wrappers.

### Star Detection Module

- [x] Add `estimate_background_image(image: &AstroImage, tile_size: usize) -> BackgroundMap` wrapper in `src/star_detection/background.rs`
- [x] Add `estimate_background_iterative_image(image: &AstroImage, tile_size: usize, config: &IterativeBackgroundConfig) -> BackgroundMap` wrapper
- [x] Export both in `src/star_detection/mod.rs` and `src/lib.rs`

### Stacking Module

- [x] Add `remove_gradient_image(image: &AstroImage, config: &GradientRemovalConfig) -> Result<AstroImage, GradientRemovalError>` in `src/stacking/gradient_removal.rs`
- [x] Add `LiveStackAccumulator::from_reference(reference: &AstroImage, config: LiveStackConfig) -> Result<Self, LiveStackError>` in `src/stacking/live.rs`
- [x] Export both in `src/stacking/mod.rs` and `src/lib.rs`

### Registration Module

- [x] Verify `warp_to_reference` handles multi-channel AstroImage correctly, fix if needed
- [x] Add `quick_register(ref_stars: &[Star], target_stars: &[Star]) -> Result<RegistrationResult, RegistrationError>` convenience function with sensible defaults (affine, standard RANSAC)
- [x] Export in `src/registration/mod.rs` and `src/lib.rs`

## Phase 2: Add AstroImage convenience methods

- [x] Add `AstroImage::save(&self, path: impl AsRef<Path>) -> Result<(), Error>` method that handles conversion to imaginarium::Image internally
- [x] Add `AstroImage::get_pixel(&self, x: usize, y: usize) -> f32` for single-channel images (clearer than `get_pixel_gray`)
- [x] Add `AstroImage::get_pixel_channel(&self, x: usize, y: usize, channel: usize) -> f32` for any channel access
- [x] Deprecate `get_pixel_gray` with message pointing to `get_pixel`

## Phase 3: Constructor pattern consistency

- [x] Add `LiveStackConfig::builder() -> LiveStackConfigBuilder` with methods: `running_mean()`, `weighted_mean()`, `rolling_sigma_clip(window, sigma)`, `normalize(bool)`, `track_variance(bool)`, `build()`
- [x] Add `CalibrationMasters::load()` as clearer alias for `load_from_directory()`
- [x] Add `CalibrationMasters::create()` as clearer alias for `from_directory()`
- [x] Deprecate old `load_from_directory` and `from_directory` names

## Phase 4: Naming consistency

- [x] Rename `register_stars` free function to `register_star_positions` (avoids confusion with `Registrator::register_stars` method)
- [x] Keep old name as deprecated alias
- [x] Rename `StarDetectionConfig.iterative_background_passes` field to `background_passes` (redundant prefix)

## Phase 5: Update exports

- [x] Update `src/prelude.rs` to export all new wrapper functions and builders
- [x] Ensure all public API is documented with examples

## Phase 6: Examples

Create examples that cover all major library use cases. Each example should:
- Be simple and easy to understand
- Show a complete workflow pipeline
- Use the convenient high-level API
- If API is inconvenient for an example, add missing functionality to the library first

Existing: `examples/full_pipeline.rs` (calibration + registration + stacking)

### Example 1: Star Detection (`examples/star_detection.rs`)

- [ ] Create example showing star detection workflow:
  - Load image with `AstroImage::from_file()`
  - Detect stars with default config: `find_stars(&image, &StarDetectionConfig::default())`
  - Detect with custom config using builder: `StarDetectionConfig::builder().for_wide_field().with_min_snr(15.0).build()`
  - Print star properties (position, flux, FWHM, SNR) and diagnostics
  - If any API is awkward, fix it first

### Example 2: Quick Alignment (`examples/quick_align.rs`)

- [ ] Create example showing simple 2-image alignment:
  - Load reference and target images
  - Detect stars in both
  - Align with `quick_register()` (requires Phase 1 completion)
  - Warp target to reference with `warp_to_reference()`
  - Save result with `image.save()` (requires Phase 2 completion)
  - Should be under 30 lines of main logic

### Example 3: Live Stacking (`examples/live_stacking.rs`)

- [ ] Create example showing real-time EAA workflow:
  - Create accumulator with `LiveStackAccumulator::from_reference()` (requires Phase 1)
  - Configure with `LiveStackConfig::builder()` (requires Phase 3)
  - Simulate incoming frames in a loop
  - Compute frame quality from star detection
  - Add frames and show SNR improvement
  - Get preview for display
  - Finalize and save result

### Example 4: Multi-Session Stacking (`examples/multi_session.rs`)

- [ ] Create example showing combining data from multiple nights:
  - Create `Session` objects with frame paths
  - Assess quality with `session.assess_quality()`
  - Create `MultiSessionStack` with sessions
  - Configure with `SessionConfig::default().with_quality_threshold(0.3)`
  - Filter low-quality frames
  - Stack with `stack_session_weighted()`
  - Print session contributions
  - Save result

### Example 5: Gradient Removal (`examples/gradient_removal.rs`)

- [ ] Create example showing light pollution correction:
  - Load stacked image with gradient
  - Remove with defaults: `remove_gradient_image(&image, &GradientRemovalConfig::default())` (requires Phase 1)
  - Show polynomial model: `GradientModel::Polynomial(2)`
  - Show RBF model: `GradientModel::Rbf(0.5)`
  - Show subtract vs divide correction methods
  - Save corrected results

### Example 6: Comet Stacking (`examples/comet_stacking.rs`)

- [ ] Create example showing moving object tracking:
  - Define comet positions at start/end with `ObjectPosition::new()`
  - Configure with `CometStackConfig::new(start, end)`
  - Load frames with timestamps
  - Compute offsets with `compute_comet_offset()`
  - Create star-tracked and comet-tracked stacks
  - Composite with `composite_stacks()`
  - If workflow is awkward, consider adding `CometStacker` unified API

### Example 7: Plate Solving (`examples/plate_solve.rs`)

- [ ] Create example showing astrometry workflow:
  - Load image and detect stars
  - Configure `PlateSolver` with approximate coordinates and scale
  - Solve to get WCS
  - Convert pixel to sky coordinates
  - Convert sky to pixel coordinates
  - Print solution quality (RMS, rotation, scale)

## Verification

Run each example and verify it works as intended.

After each task run:
```
cargo nextest run -p lumos && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
```
