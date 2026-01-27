# Lumos Public API Review

> Generated: 2026-01-27

This document identifies inconsistencies in the lumos public API and provides a plan to address them.

## Summary of Issues

| Category | Severity | Count |
|----------|----------|-------|
| Functions taking raw pixels/width/height | High | 14 |
| Inconsistent constructor patterns | Medium | 5 |
| Naming inconsistencies | Medium | 6 |
| Complex APIs needing simplification | Low | 5 |

---

## Issue 1: Functions Taking Raw Pixels/Width/Height Instead of AstroImage

**Problem**: Many functions accept `(pixels: &[f32], width: usize, height: usize)` instead of `&AstroImage`. This creates API inconsistency and forces users to manually destructure images.

### Affected Functions

#### Star Detection Module
| Function | Current Signature |
|----------|-------------------|
| `estimate_background` | `(pixels: &[f32], width: usize, height: usize, tile_size: usize)` |
| `estimate_background_iterative` | `(pixels: &[f32], width: usize, height: usize, tile_size: usize, config: &IterativeBackgroundConfig)` |
| `fit_gaussian_2d` | `(pixels: &[f32], width: usize, height: usize, ...)` |
| `fit_moffat_2d` | `(pixels: &[f32], width: usize, height: usize, ...)` |

#### Registration Module
| Function | Current Signature |
|----------|-------------------|
| `Registrator::register_with_phase_correlation` | `(ref_image: &[f32], target_image: &[f32], width: usize, height: usize, ...)` |
| `PhaseCorrelator::new` | `(width: usize, height: usize, config: PhaseCorrelationConfig)` |
| `PhaseCorrelator::correlate` | `(ref_image: &[f32], target_image: &[f32], width: usize, height: usize)` |
| `warp_image` | `(pixels: &[f32], width: usize, height: usize, transform: &TransformMatrix, config: &WarpConfig)` |
| `interpolate_pixel` | `(data: &[f32], width: usize, height: usize, x: f32, y: f32, config: &WarpConfig)` |
| `check_quadrant_consistency` | `(transform: &TransformMatrix, width: usize, height: usize)` |
| `estimate_overlap` | `(transform: &TransformMatrix, width: usize, height: usize)` |

#### Stacking Module
| Function | Current Signature |
|----------|-------------------|
| `remove_gradient` | `(pixels: &[f32], width: usize, height: usize, config: &GradientRemovalConfig)` |
| `remove_gradient_simple` | `(pixels: &[f32], width: usize, height: usize)` |
| `LiveStackAccumulator::new` | `(width: usize, height: usize, channels: usize, config: LiveStackConfig)` |

### Plan to Address

**Approach**: Add `AstroImage`-based wrapper functions while keeping low-level functions for advanced use cases.

#### Step 1: Add wrapper functions to each module

```rust
// star_detection/background.rs
pub fn estimate_background_image(
    image: &AstroImage,
    tile_size: usize,
) -> BackgroundMap {
    estimate_background(image.pixels(), image.width(), image.height(), tile_size)
}

pub fn estimate_background_iterative_image(
    image: &AstroImage,
    tile_size: usize,
    config: &IterativeBackgroundConfig,
) -> BackgroundMap {
    estimate_background_iterative(image.pixels(), image.width(), image.height(), tile_size, config)
}
```

```rust
// registration/warp.rs
pub fn warp_image_astro(
    image: &AstroImage,
    transform: &TransformMatrix,
    config: &WarpConfig,
) -> AstroImage {
    // Already exists as warp_to_reference - ensure it's exported
}
```

```rust
// stacking/gradient_removal.rs
pub fn remove_gradient_image(
    image: &AstroImage,
    config: &GradientRemovalConfig,
) -> Result<AstroImage, GradientRemovalError> {
    // Per-channel processing, returns AstroImage
}
```

#### Step 2: Update LiveStackAccumulator constructor

```rust
// Current
pub fn new(width: usize, height: usize, channels: usize, config: LiveStackConfig) -> Result<Self, LiveStackError>

// Add alternative
pub fn from_reference(reference: &AstroImage, config: LiveStackConfig) -> Result<Self, LiveStackError> {
    Self::new(reference.width(), reference.height(), reference.channels(), config)
}
```

#### Step 3: Update prelude to export new functions

Add all `*_image` variants to `src/prelude.rs`.

---

## Issue 2: Inconsistent Constructor Patterns

**Problem**: Different types use different construction patterns, creating cognitive load.

| Type | Pattern | Method |
|------|---------|--------|
| `StarDetectionConfig` | Builder | `StarDetectionConfig::builder().build()` |
| `RegistrationConfig` | Builder | `RegistrationConfig::builder().build()` |
| `LiveStackConfig` | Default | `LiveStackConfig::default()` with field mutation |
| `CalibrationMasters` | Factory | `load_from_directory()` vs `from_directory()` |
| `ImageStack` | Constructor | `ImageStack::new(...)` |
| `Session` | Constructor + Builder | `Session::new(id).with_frames(...)` |

### Plan to Address

#### Step 1: Add builder to LiveStackConfig

```rust
impl LiveStackConfig {
    pub fn builder() -> LiveStackConfigBuilder {
        LiveStackConfigBuilder::default()
    }
}

pub struct LiveStackConfigBuilder {
    mode: LiveStackMode,
    normalize: bool,
    preview_channel: Option<usize>,
    track_variance: bool,
}

impl LiveStackConfigBuilder {
    pub fn running_mean(mut self) -> Self { self.mode = LiveStackMode::RunningMean; self }
    pub fn weighted_mean(mut self) -> Self { self.mode = LiveStackMode::WeightedMean; self }
    pub fn rolling_sigma_clip(mut self, window_size: usize, sigma: f32) -> Self {
        self.mode = LiveStackMode::RollingSigmaClip { window_size, sigma };
        self
    }
    pub fn normalize(mut self, enable: bool) -> Self { self.normalize = enable; self }
    pub fn preview_channel(mut self, channel: Option<usize>) -> Self { self.preview_channel = channel; self }
    pub fn track_variance(mut self, enable: bool) -> Self { self.track_variance = enable; self }
    pub fn build(self) -> LiveStackConfig { ... }
}
```

#### Step 2: Clarify CalibrationMasters factory methods

```rust
// Rename for clarity
impl CalibrationMasters {
    /// Load existing master frames from directory
    pub fn load(dir: impl AsRef<Path>, method: StackingMethod) -> Result<Self>
    
    /// Create master frames from raw calibration files
    pub fn create(dir: impl AsRef<Path>, method: StackingMethod, progress: ProgressCallback) -> Result<Self>
    
    // Deprecate old names
    #[deprecated(note = "Use `load()` instead")]
    pub fn load_from_directory(...) -> Result<Self> { Self::load(...) }
    
    #[deprecated(note = "Use `create()` instead")]
    pub fn from_directory(...) -> Result<Self> { Self::create(...) }
}
```

---

## Issue 3: Naming Inconsistencies

### 3.1 Pixel Access Methods

**Current**:
- `pixels()` - Returns all pixels
- `get_pixel_gray(x, y)` - Single channel access
- `get_pixel_rgb(x, y)` - Multi-channel access

**Problem**: `gray` implies grayscale conversion, but it just accesses single-channel data.

**Plan**: Rename for clarity

```rust
// Deprecate
#[deprecated(note = "Use `get_pixel(x, y)` instead")]
pub fn get_pixel_gray(&self, x: usize, y: usize) -> f32

// Add clearer alternatives
pub fn get_pixel(&self, x: usize, y: usize) -> f32  // For grayscale images
pub fn get_pixel_channel(&self, x: usize, y: usize, channel: usize) -> f32  // Any channel
```

### 3.2 Background Estimation

**Current**:
- `estimate_background()` - Single pass
- `estimate_background_iterative()` - Multiple passes
- `IterativeBackgroundConfig` has `iterative_background_passes` field

**Problem**: Redundant naming - "iterative" appears twice.

**Plan**: Simplify

```rust
// Keep both functions but clarify in docs
/// Single-pass background estimation. For iterative refinement, use `estimate_background_iterative`.
pub fn estimate_background(...)

/// Iterative background estimation with source masking refinement.
pub fn estimate_background_iterative(...)

// Rename config field
pub struct IterativeBackgroundConfig {
    pub passes: u32,  // was: iterative_background_passes
    // ...
}
```

### 3.3 Registration Functions

**Current**:
- `register_stars(ref_stars: &[(f64, f64)], ...)` - Free function taking tuples
- `Registrator::register_stars(ref_stars: &[Star], ...)` - Method taking Star objects
- `Registrator::register_positions(ref_positions: &[(f64, f64)], ...)` - Method taking tuples

**Problem**: Same function name, different signatures; confusing which to use.

**Plan**: Rename free function

```rust
// Rename free function to be distinct
pub fn register_star_positions(
    ref_positions: &[(f64, f64)],
    target_positions: &[(f64, f64)],
    transform_type: TransformType,
) -> Result<RegistrationResult, RegistrationError>

// Deprecate old name
#[deprecated(note = "Use `register_star_positions()` instead")]
pub fn register_stars(...) -> Result<RegistrationResult, RegistrationError>
```

---

## Issue 4: Complex APIs Needing Simplification

### 4.1 StarDetectionConfig (30+ fields)

**Problem**: Overwhelming number of configuration options.

**Plan**: Group into logical sub-configs

```rust
pub struct StarDetectionConfig {
    pub detection: DetectionParams,
    pub filtering: FilteringParams,
    pub centroid: CentroidParams,
    pub background: BackgroundParams,
    pub deblending: DeblendParams,
}

pub struct DetectionParams {
    pub sigma: f32,
    pub min_area: usize,
    pub max_area: usize,
    pub edge_margin: usize,
}

pub struct FilteringParams {
    pub min_snr: f32,
    pub max_eccentricity: f32,
    pub max_sharpness: f32,
    pub max_roundness: f32,
    pub max_fwhm_deviation: f32,
}
// ... etc
```

**Note**: This is a larger refactor. Consider for v2.0.

### 4.2 Comet Stacking API

**Current**: Four separate functions that must be called in sequence.

**Plan**: Create unified `CometStacker` type

```rust
pub struct CometStacker {
    config: CometStackConfig,
}

impl CometStacker {
    pub fn new(config: CometStackConfig) -> Self { ... }
    
    /// Stack frames tracking comet motion
    pub fn stack(
        &self,
        frames: &[AstroImage],
        comet_positions: &[(f64, f64, f64)],  // (x, y, timestamp)
    ) -> CometStackResult { ... }
}

pub struct CometStackResult {
    pub comet_stack: AstroImage,      // Comet-tracked stack
    pub star_stack: AstroImage,       // Star-tracked stack  
    pub composite: AstroImage,        // Combined result
}
```

---

## Issue 5: Error Handling Inconsistency

**Problem**: Some validation functions panic, others return `Result`.

| Function | Behavior |
|----------|----------|
| `StarDetectionConfig::validate()` | Panics |
| `RegistrationConfig::validate()` | Panics |
| `ImageStack::process()` | Returns `Result` |
| `LiveStackAccumulator::new()` | Returns `Result` |

**Plan**: Standardize on `Result` for public APIs

```rust
impl StarDetectionConfig {
    /// Validate configuration. Returns error if invalid.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.detection_sigma <= 0.0 {
            return Err(ConfigError::InvalidParameter {
                name: "detection_sigma",
                reason: "must be positive",
            });
        }
        // ...
        Ok(())
    }
}

impl StarDetectionConfigBuilder {
    /// Build config, returning error if validation fails.
    pub fn build(self) -> Result<StarDetectionConfig, ConfigError> {
        let config = StarDetectionConfig { ... };
        config.validate()?;
        Ok(config)
    }
}
```

---

## Implementation Priority

### Phase 1: High Impact, Low Risk
1. Add `*_image` wrapper functions for raw pixel APIs
2. Add `LiveStackAccumulator::from_reference()`
3. Rename `CalibrationMasters` factory methods

### Phase 2: Medium Impact
4. Add `LiveStackConfigBuilder`
5. Rename `register_stars` free function to `register_star_positions`
6. Deprecate `get_pixel_gray` in favor of `get_pixel`

### Phase 3: Larger Refactors (Consider for v2.0)
7. Restructure `StarDetectionConfig` into sub-configs
8. Create unified `CometStacker` API
9. Standardize error handling (panic → Result)

---

## Files to Modify

| File | Changes |
|------|---------|
| `src/star_detection/background.rs` | Add `estimate_background_image()` |
| `src/star_detection/profile_fitting.rs` | Add image-based wrappers |
| `src/registration/warp.rs` | Ensure `warp_to_reference` is exported |
| `src/registration/phase_correlation.rs` | Add image-based wrappers |
| `src/registration/mod.rs` | Rename `register_stars` → `register_star_positions` |
| `src/stacking/gradient_removal.rs` | Add `remove_gradient_image()` |
| `src/stacking/live.rs` | Add builder, add `from_reference()` |
| `src/calibration_masters.rs` | Rename factory methods |
| `src/astro_image/mod.rs` | Rename pixel access methods |
| `src/prelude.rs` | Export new functions |

---

## Backward Compatibility

All changes should:
1. Add new APIs alongside existing ones
2. Mark old APIs as `#[deprecated]` with migration guidance
3. Keep deprecated APIs for at least one minor version
4. Document changes in CHANGELOG

Example deprecation:

```rust
#[deprecated(
    since = "0.X.0",
    note = "Use `estimate_background_image()` for AstroImage input, \
            or keep using `estimate_background()` for raw pixel data"
)]
pub fn old_function(...) { ... }
```

---

## Issue 6: Examples Coverage

**Problem**: Only one example (`full_pipeline.rs`) exists. It's comprehensive but covers only the traditional calibration+registration+stacking workflow. Many important use cases are not demonstrated.

### Current State

| Example | Coverage |
|---------|----------|
| `full_pipeline.rs` | Calibration masters, light calibration, star detection, registration, stacking |

### Missing Use Cases

| Use Case | Priority | Notes |
|----------|----------|-------|
| Live stacking (real-time preview) | High | Key feature for EAA (Electronically Assisted Astronomy) |
| Multi-session stacking | High | Combining data from multiple nights |
| Comet/asteroid stacking | Medium | Moving object tracking |
| Star detection only | Medium | Users may only need star analysis |
| Plate solving (astrometry) | Medium | WCS coordinate mapping |
| Gradient removal | Medium | Light pollution correction |
| Quick registration | Low | Simple 2-image alignment without full pipeline |
| Distortion correction | Low | Lens/optical distortion models |

### Plan: Create Example Files

#### Example 1: `examples/star_detection.rs` (Simple, Focused)

**Purpose**: Demonstrate star detection in isolation.

```rust
//! Example: Star Detection
//!
//! Demonstrates finding and analyzing stars in an astronomical image.

use lumos::prelude::*;

fn main() -> anyhow::Result<()> {
    // Load image
    let image = AstroImage::from_file("image.fits")?;
    
    // Basic detection with defaults
    let result = find_stars(&image, &StarDetectionConfig::default());
    println!("Found {} stars", result.stars.len());
    
    // Detection with custom config via builder
    let config = StarDetectionConfig::builder()
        .for_wide_field()
        .with_min_snr(15.0)
        .with_cosmic_ray_rejection(0.7)
        .build();
    
    let result = find_stars(&image, &config);
    
    // Print diagnostics
    println!("Median FWHM: {:.2} pixels", result.diagnostics.median_fwhm);
    println!("Median SNR: {:.1}", result.diagnostics.median_snr);
    
    // Iterate over stars
    for star in result.stars.iter().take(10) {
        println!(
            "Star at ({:.1}, {:.1}): flux={:.0}, FWHM={:.2}, SNR={:.1}",
            star.x, star.y, star.flux, star.fwhm, star.snr
        );
    }
    
    Ok(())
}
```

#### Example 2: `examples/live_stacking.rs` (Real-Time Preview)

**Purpose**: Demonstrate live stacking for EAA sessions.

**Required API Addition**: `LiveStackAccumulator::from_reference()` for simpler construction.

```rust
//! Example: Live Stacking for Real-Time Preview
//!
//! Demonstrates incremental frame stacking with quality monitoring,
//! suitable for Electronically Assisted Astronomy (EAA).

use lumos::prelude::*;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    // Simulate incoming frames (in real use, these come from camera)
    let frame_paths = vec!["frame_001.fits", "frame_002.fits", "frame_003.fits"];
    
    // Load first frame to get dimensions
    let first_frame = AstroImage::from_file(&frame_paths[0])?;
    
    // Configure live stacking
    let config = LiveStackConfig {
        mode: LiveStackMode::WeightedMean,
        normalize: true,
        track_variance: true,
        ..Default::default()
    };
    
    // Create accumulator from reference frame dimensions
    let mut accumulator = LiveStackAccumulator::from_reference(&first_frame, config)?;
    
    // Process frames as they arrive
    for path in &frame_paths {
        let frame = AstroImage::from_file(path)?;
        
        // Compute frame quality (in real use, from star detection)
        let quality = compute_frame_quality(&frame);
        
        // Add frame and get updated stats
        let stats = accumulator.add_frame(&frame, quality)?;
        
        println!(
            "Frame {}: SNR improvement {:.1}x, mean FWHM {:.2}",
            stats.frame_count,
            stats.snr_improvement,
            stats.mean_fwhm
        );
        
        // Get preview for display (cheap operation)
        let preview = accumulator.preview()?;
        display_preview(&preview);
    }
    
    // Finalize when done
    let result = accumulator.finalize()?;
    result.image.save("live_stack_result.tiff")?;
    
    Ok(())
}

fn compute_frame_quality(image: &AstroImage) -> LiveFrameQuality {
    let config = StarDetectionConfig::builder()
        .for_wide_field()
        .with_min_snr(10.0)
        .build();
    
    let result = find_stars(image, &config);
    
    LiveFrameQuality {
        snr: result.diagnostics.median_snr,
        fwhm: result.diagnostics.median_fwhm,
        eccentricity: result.diagnostics.median_eccentricity,
        noise: result.diagnostics.background_rms,
        star_count: result.stars.len(),
    }
}

fn display_preview(_image: &AstroImage) {
    // In real application, render to screen
    println!("  (preview updated)");
}
```

#### Example 3: `examples/multi_session.rs` (Combining Multiple Nights)

**Purpose**: Demonstrate combining data from multiple imaging sessions.

```rust
//! Example: Multi-Session Stacking
//!
//! Demonstrates combining data from multiple imaging sessions
//! (e.g., different nights) with automatic quality weighting.

use lumos::prelude::*;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    // Define sessions (typically from different nights)
    let session1_frames: Vec<PathBuf> = (1..=10)
        .map(|i| format!("night1/frame_{:03}.fits", i).into())
        .collect();
    
    let session2_frames: Vec<PathBuf> = (1..=15)
        .map(|i| format!("night2/frame_{:03}.fits", i).into())
        .collect();
    
    // Create sessions with quality assessment
    let star_config = StarDetectionConfig::builder()
        .for_wide_field()
        .with_min_snr(10.0)
        .build();
    
    let session1 = Session::new("night1")
        .with_frames(&session1_frames)
        .assess_quality(&star_config)?;
    
    let session2 = Session::new("night2")
        .with_frames(&session2_frames)
        .assess_quality(&star_config)?;
    
    // Configure multi-session stacking
    let config = SessionConfig::default()
        .with_quality_threshold(0.3);  // Reject worst 30%
    
    // Create stack
    let mut multi_stack = MultiSessionStack::new(vec![session1, session2])
        .with_config(config);
    
    // Print summary before stacking
    let summary = multi_stack.summary();
    println!("{}", summary);
    
    // Filter low-quality frames
    let rejected = multi_stack.filter_all_frames();
    println!("Rejected {} low-quality frames", rejected.len());
    
    // Stack with session-aware weighting
    let result = multi_stack.stack_session_weighted()?;
    
    // Print contributions
    for (idx, weight) in result.session_contributions(&multi_stack) {
        println!("Session {} contributed {:.1}%", idx, weight * 100.0);
    }
    
    // Save result
    result.image().save("multi_session_result.tiff")?;
    
    Ok(())
}
```

#### Example 4: `examples/comet_stacking.rs` (Moving Objects)

**Purpose**: Demonstrate tracking and stacking moving objects.

**Required API Addition**: Simplified `CometStacker` API (see Issue 4.2).

```rust
//! Example: Comet/Asteroid Stacking
//!
//! Demonstrates stacking frames while tracking a moving object,
//! producing both comet-tracked and star-tracked results.

use lumos::prelude::*;

fn main() -> anyhow::Result<()> {
    // Frame paths with timestamps (from FITS headers or filename)
    let frames_with_times = vec![
        ("frame_001.fits", 0.0),      // t=0 minutes
        ("frame_002.fits", 2.0),      // t=2 minutes  
        ("frame_003.fits", 4.0),      // t=4 minutes
        // ... more frames
    ];
    
    // Comet positions at start and end of session
    // (measured manually or from orbital elements)
    let comet_start = ObjectPosition::new(512.3, 384.7, 0.0);
    let comet_end = ObjectPosition::new(518.1, 382.2, 60.0);  // After 60 minutes
    
    // Configure comet stacking
    let config = CometStackConfig::new(comet_start, comet_end)
        .rejection(RejectionMethod::SigmaClip { sigma: 2.5 })
        .composite_method(CompositeMethod::Blend { comet_weight: 0.7 });
    
    println!(
        "Comet velocity: {:.2} px/min, total displacement: {:.1} px",
        config.velocity().0.hypot(config.velocity().1),
        config.total_displacement()
    );
    
    // Load and process frames
    let mut star_frames = Vec::new();
    let mut comet_frames = Vec::new();
    
    for (path, timestamp) in &frames_with_times {
        let frame = AstroImage::from_file(path)?;
        
        // Compute comet offset for this timestamp
        let offset = compute_comet_offset(&config, *timestamp);
        
        // For star stack: use original frame
        star_frames.push(frame.clone());
        
        // For comet stack: shift frame to align comet
        let shifted = shift_image(&frame, -offset.0, -offset.1);
        comet_frames.push(shifted);
    }
    
    // Stack both versions
    let star_stack = stack_frames(&star_frames)?;
    let comet_stack = stack_frames(&comet_frames)?;
    
    // Create composite (sharp stars + sharp comet)
    let composite = composite_stacks(
        &star_stack,
        &comet_stack,
        &config,
    );
    
    // Save results
    star_stack.save("comet_stars.tiff")?;
    comet_stack.save("comet_tracked.tiff")?;
    composite.save("comet_composite.tiff")?;
    
    Ok(())
}

fn shift_image(image: &AstroImage, dx: f64, dy: f64) -> AstroImage {
    let transform = TransformMatrix::translation(dx, dy);
    warp_to_reference(image, &transform, InterpolationMethod::Lanczos3)
}

fn stack_frames(frames: &[AstroImage]) -> anyhow::Result<AstroImage> {
    // Simple sigma-clipped stacking
    // ... implementation
    todo!()
}
```

**Note**: The comet example reveals that the current API requires manual frame shifting. The `CometStacker` simplification (Issue 4.2) would make this much cleaner.

#### Example 5: `examples/plate_solve.rs` (Astrometry)

**Purpose**: Demonstrate WCS coordinate computation.

```rust
//! Example: Plate Solving (Astrometry)
//!
//! Demonstrates computing WCS (World Coordinate System) solution
//! to map pixel coordinates to sky coordinates (RA/Dec).

use lumos::prelude::*;

fn main() -> anyhow::Result<()> {
    // Load image and detect stars
    let image = AstroImage::from_file("image.fits")?;
    let config = StarDetectionConfig::builder()
        .for_wide_field()
        .with_min_snr(20.0)  // Higher SNR for plate solving
        .build();
    
    let result = find_stars(&image, &config);
    let image_stars: Vec<(f64, f64)> = result.stars
        .iter()
        .take(100)  // Use brightest stars
        .map(|s| (s.x, s.y))
        .collect();
    
    // Configure plate solver
    // Approximate center and scale from FITS header or user input
    let solver_config = PlateSolverConfig {
        catalog: CatalogSource::GaiaVizier {
            center_ra: 83.82,   // Orion Nebula approximate
            center_dec: -5.39,
            radius_deg: 2.0,
        },
        scale_hint: Some(1.5),  // arcsec/pixel estimate
        ..Default::default()
    };
    
    let solver = PlateSolver::new(solver_config);
    
    // Solve
    match solver.solve(&image_stars, image.width(), image.height()) {
        Ok(solution) => {
            println!("Plate solution found!");
            println!("  RMS error: {:.3} arcsec", solution.rms_error * 3600.0);
            println!("  Matched stars: {}", solution.matched_count);
            println!("  Scale: {:.4} arcsec/pixel", solution.wcs.scale() * 3600.0);
            println!("  Rotation: {:.2}°", solution.wcs.rotation().to_degrees());
            
            // Convert pixel to sky coordinates
            let (ra, dec) = solution.wcs.pixel_to_sky(512.0, 384.0);
            println!("  Center: RA={:.4}°, Dec={:.4}°", ra, dec);
            
            // Convert sky to pixel
            let (x, y) = solution.wcs.sky_to_pixel(83.82, -5.39);
            println!("  M42 at pixel: ({:.1}, {:.1})", x, y);
        }
        Err(e) => {
            println!("Plate solving failed: {}", e);
        }
    }
    
    Ok(())
}
```

#### Example 6: `examples/gradient_removal.rs` (Light Pollution)

**Purpose**: Demonstrate removing sky gradients from images.

**Required API Addition**: `remove_gradient_image()` wrapper.

```rust
//! Example: Gradient Removal
//!
//! Demonstrates removing light pollution gradients from stacked images.

use lumos::prelude::*;

fn main() -> anyhow::Result<()> {
    // Load stacked image with gradient
    let image = AstroImage::from_file("stacked_with_gradient.tiff")?;
    
    // Simple gradient removal with defaults
    let corrected_simple = remove_gradient_image(&image, &GradientRemovalConfig::default())?;
    corrected_simple.save("corrected_simple.tiff")?;
    
    // Advanced: polynomial model for smooth gradients
    let config_poly = GradientRemovalConfig {
        model: GradientModel::Polynomial(2),  // Quadratic surface
        correction: CorrectionMethod::Subtract,
        sample_points: 64,
        ..Default::default()
    };
    let corrected_poly = remove_gradient_image(&image, &config_poly)?;
    corrected_poly.save("corrected_polynomial.tiff")?;
    
    // Advanced: RBF for complex gradients
    let config_rbf = GradientRemovalConfig {
        model: GradientModel::Rbf(0.5),  // Thin-plate spline
        correction: CorrectionMethod::Subtract,
        sample_points: 128,
        ..Default::default()
    };
    let corrected_rbf = remove_gradient_image(&image, &config_rbf)?;
    corrected_rbf.save("corrected_rbf.tiff")?;
    
    // For vignetting (multiplicative), use Divide
    let config_vignette = GradientRemovalConfig {
        model: GradientModel::Polynomial(4),
        correction: CorrectionMethod::Divide,
        ..Default::default()
    };
    let corrected_vignette = remove_gradient_image(&image, &config_vignette)?;
    corrected_vignette.save("corrected_vignette.tiff")?;
    
    Ok(())
}
```

#### Example 7: `examples/quick_align.rs` (Simple Registration)

**Purpose**: Minimal example for aligning two images.

```rust
//! Example: Quick Image Alignment
//!
//! Demonstrates the simplest way to align two images.

use lumos::prelude::*;

fn main() -> anyhow::Result<()> {
    // Load reference and target images
    let reference = AstroImage::from_file("reference.fits")?;
    let target = AstroImage::from_file("target.fits")?;
    
    // Detect stars in both
    let config = StarDetectionConfig::default();
    let ref_stars = find_stars(&reference, &config);
    let target_stars = find_stars(&target, &config);
    
    println!(
        "Reference: {} stars, Target: {} stars",
        ref_stars.stars.len(),
        target_stars.stars.len()
    );
    
    // Quick registration (uses sensible defaults)
    let result = quick_register(&ref_stars.stars, &target_stars.stars)?;
    
    println!(
        "Aligned with {} matched stars, RMS error: {:.3} px",
        result.num_inliers,
        result.rms_error
    );
    
    // Warp target to match reference
    let aligned = warp_to_reference(&target, &result.transform, InterpolationMethod::Lanczos3);
    aligned.save("aligned.tiff")?;
    
    Ok(())
}
```

### Required API Additions for Examples

| Addition | For Example | Description |
|----------|-------------|-------------|
| `LiveStackAccumulator::from_reference()` | live_stacking | Simpler construction from AstroImage |
| `remove_gradient_image()` | gradient_removal | AstroImage wrapper for gradient removal |
| `AstroImage::save()` | All | Direct save method (currently requires conversion to imaginarium::Image) |
| `quick_register()` | quick_align | Convenience function with sensible defaults |
| `CometStacker` (unified API) | comet_stacking | Simplified comet stacking workflow |

### API Improvements Discovered While Designing Examples

#### 6.1 Missing `AstroImage::save()` Method

**Current**: Users must convert to `imaginarium::Image` before saving:
```rust
let img: imaginarium::Image = astro_image.into();
img.save_file(&path)?;
```

**Proposed**: Add direct save method:
```rust
impl AstroImage {
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ImageError> {
        let img: imaginarium::Image = self.clone().into();
        img.save_file(path.as_ref()).map_err(ImageError::Io)
    }
}
```

#### 6.2 Missing `quick_register()` Convenience Function

**Purpose**: One-liner registration for common cases.

```rust
/// Quick registration with sensible defaults.
///
/// Uses affine transform with default RANSAC parameters.
/// For more control, use `Registrator` directly.
pub fn quick_register(
    ref_stars: &[Star],
    target_stars: &[Star],
) -> Result<RegistrationResult, RegistrationError> {
    let config = RegistrationConfig::builder()
        .full_affine()
        .build();
    Registrator::new(config).register_stars(ref_stars, target_stars)
}
```

#### 6.3 `warp_to_reference` Should Accept `&AstroImage`

**Current**: Takes raw pixels and returns raw pixels.

**Problem**: Inconsistent with other high-level APIs.

**Proposed**: The current `warp_to_reference` in `full_pipeline.rs` example is actually a custom function. The library should provide:

```rust
pub fn warp_to_reference(
    image: &AstroImage,
    transform: &TransformMatrix,
    method: InterpolationMethod,
) -> AstroImage
```

This appears to exist but needs verification it handles multi-channel correctly.

---

## Implementation Priority (Updated)

### Phase 1: High Impact, Low Risk
1. Add `*_image` wrapper functions for raw pixel APIs
2. Add `LiveStackAccumulator::from_reference()`
3. Rename `CalibrationMasters` factory methods
4. **Add `AstroImage::save()` method**
5. **Add `quick_register()` convenience function**

### Phase 2: Medium Impact
6. Add `LiveStackConfigBuilder`
7. Rename `register_stars` free function to `register_star_positions`
8. Deprecate `get_pixel_gray` in favor of `get_pixel`
9. **Create example files** (star_detection, live_stacking, multi_session, gradient_removal, quick_align)

### Phase 3: Larger Refactors (Consider for v2.0)
10. Restructure `StarDetectionConfig` into sub-configs
11. Create unified `CometStacker` API
12. Standardize error handling (panic → Result)
13. **Create remaining examples** (comet_stacking, plate_solve)

---

## Files to Modify (Updated)

| File | Changes |
|------|---------|
| `src/star_detection/background.rs` | Add `estimate_background_image()` |
| `src/star_detection/profile_fitting.rs` | Add image-based wrappers |
| `src/registration/warp.rs` | Ensure `warp_to_reference` is exported, handles AstroImage |
| `src/registration/phase_correlation.rs` | Add image-based wrappers |
| `src/registration/mod.rs` | Rename `register_stars` → `register_star_positions`, add `quick_register()` |
| `src/stacking/gradient_removal.rs` | Add `remove_gradient_image()` |
| `src/stacking/live.rs` | Add builder, add `from_reference()` |
| `src/calibration_masters.rs` | Rename factory methods |
| `src/astro_image/mod.rs` | Rename pixel access methods, **add `save()` method** |
| `src/prelude.rs` | Export new functions |
| **`examples/star_detection.rs`** | **New file** |
| **`examples/live_stacking.rs`** | **New file** |
| **`examples/multi_session.rs`** | **New file** |
| **`examples/gradient_removal.rs`** | **New file** |
| **`examples/quick_align.rs`** | **New file** |
| **`examples/comet_stacking.rs`** | **New file (Phase 3)** |
| **`examples/plate_solve.rs`** | **New file (Phase 3)** |
