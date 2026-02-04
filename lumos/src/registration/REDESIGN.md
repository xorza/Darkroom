# Registration Module API Redesign Proposal

## Executive Summary

This proposal recommends flattening the registration module's configuration hierarchy and simplifying the public API to match industry best practices. The current API has 5 nested config structs with 25+ parameters. The proposed API has a single flat `Config` struct with grouped fields, plus simple top-level functions for common use cases.

**Key inspirations:**
- [Astroalign](https://github.com/quatrope/astroalign): 2 main functions (`register`, `find_transform`)
- [OpenCV](https://docs.opencv.org/4.x/da/d54/group__imgproc__transform.html): Separate functions for each operation (`findHomography`, `warpPerspective`)
- [image-registration](https://image-registration.readthedocs.io/): Single entry point (`chi2_shift`)

## Current API Problems

### 1. Nested Configuration Complexity

```rust
// Current: 5 levels of nesting, 25+ parameters
let config = RegistrationConfig {
    transform_type: TransformType::Similarity,
    min_stars_for_matching: 10,
    min_matched_stars: 8,
    max_residual_pixels: 2.0,
    use_spatial_distribution: true,
    spatial_grid_size: 8,
    triangle: TriangleMatchConfig {
        max_stars: 200,
        ratio_tolerance: 0.01,
        min_votes: 3,
        check_orientation: true,
    },
    ransac: RansacConfig {
        max_iterations: 2000,
        inlier_threshold: 2.0,
        confidence: 0.995,
        min_inlier_ratio: 0.3,
        seed: None,
        use_local_optimization: true,
        lo_max_iterations: 10,
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
    },
    warp: WarpConfig {
        method: InterpolationMethod::Lanczos3,
        border_value: 0.0,
        normalize_kernel: true,
        clamp_output: false,
    },
    sip: SipCorrectionConfig {
        enabled: false,
        order: 3,
    },
};
```

**Problems:**
- Overwhelming for new users
- Hard to discover available options
- Validation spread across multiple `validate()` methods
- Interdependent parameters across structs

### 2. Stateful API with Limited Benefit

```rust
// Current: Stateful Registrator
let registrator = Registrator::new(config);
let result = registrator.register_stars(&ref_stars, &target_stars)?;
```

The `Registrator` struct holds only a config—no caches, no intermediate state. The stateful API provides no benefit over a function call.

### 3. Re-exports Explosion

The module re-exports 30+ types from `mod.rs`, making it hard to understand what the core API is.

## Proposed API

### Design Principles

1. **Flat configuration** — All parameters in a single struct with logical grouping via comments
2. **Function-first API** — Top-level functions for common operations; struct for advanced use
3. **Sensible defaults** — Users should rarely need to change anything
4. **Presets for use cases** — Named configurations for common scenarios
5. **Minimal re-exports** — Only essential types exposed

### 1. Flat Config Struct

```rust
/// Configuration for image registration.
///
/// All parameters have sensible defaults. Most users only need to set
/// `transform_type` if they want a specific model.
#[derive(Debug, Clone)]
pub struct Config {
    // == Transform model ==
    /// Transformation model: Translation, Euclidean, Similarity, Affine, Homography, Auto.
    /// Default: Auto (starts with Similarity, upgrades to Homography if needed).
    pub transform_type: TransformType,
    
    // == Star matching ==
    /// Maximum stars to use for matching (brightest N). Default: 200.
    pub max_stars: usize,
    /// Minimum stars required in each image. Default: 10.
    pub min_stars: usize,
    /// Minimum matched star pairs to accept. Default: 8.
    pub min_matches: usize,
    /// Triangle ratio tolerance (0.01 = 1%). Default: 0.01.
    pub ratio_tolerance: f64,
    /// Minimum confirming triangles per match. Default: 3.
    pub min_votes: usize,
    /// Check orientation (false allows mirrored images). Default: true.
    pub check_orientation: bool,
    
    // == Spatial distribution ==
    /// Use spatial grid for star selection. Default: true.
    pub use_spatial_grid: bool,
    /// Grid size (NxN cells). Default: 8.
    pub spatial_grid_size: usize,
    
    // == RANSAC ==
    /// RANSAC iterations. Default: 2000.
    pub ransac_iterations: usize,
    /// Inlier distance threshold in pixels. Default: 2.0.
    pub inlier_threshold: f64,
    /// Target confidence for early termination. Default: 0.995.
    pub confidence: f64,
    /// Minimum inlier ratio. Default: 0.3.
    pub min_inlier_ratio: f64,
    /// Random seed for reproducibility (None = random). Default: None.
    pub seed: Option<u64>,
    /// Enable LO-RANSAC refinement. Default: true.
    pub local_optimization: bool,
    /// LO-RANSAC iterations. Default: 10.
    pub lo_iterations: usize,
    /// Maximum rotation in radians (None = unlimited). Default: Some(0.175).
    pub max_rotation: Option<f64>,
    /// Scale range (min, max). Default: Some((0.8, 1.2)).
    pub scale_range: Option<(f64, f64)>,
    
    // == Quality ==
    /// Maximum acceptable RMS error in pixels. Default: 2.0.
    pub max_rms_error: f64,
    
    // == Distortion correction ==
    /// Enable SIP polynomial distortion correction. Default: false.
    pub sip_enabled: bool,
    /// SIP polynomial order (2-5). Default: 3.
    pub sip_order: usize,
    
    // == Image warping ==
    /// Interpolation method for warping. Default: Lanczos3.
    pub interpolation: InterpolationMethod,
    /// Border value for out-of-bounds pixels. Default: 0.0.
    pub border_value: f32,
    /// Normalize Lanczos kernel weights. Default: true.
    pub normalize_kernel: bool,
    /// Clamp output to reduce Lanczos ringing. Default: false.
    pub clamp_output: bool,
}

impl Config {
    /// Default configuration suitable for most astrophotography.
    pub fn default() -> Self { /* ... */ }
    
    /// Fast configuration: fewer iterations, lower quality, faster.
    pub fn fast() -> Self {
        Self {
            ransac_iterations: 500,
            max_stars: 100,
            local_optimization: false,
            interpolation: InterpolationMethod::Bilinear,
            ..Self::default()
        }
    }
    
    /// Precise configuration: more iterations, SIP correction enabled.
    pub fn precise() -> Self {
        Self {
            ransac_iterations: 5000,
            confidence: 0.999,
            sip_enabled: true,
            max_rms_error: 1.0,
            ..Self::default()
        }
    }
    
    /// Wide-field configuration: handles lens distortion.
    pub fn wide_field() -> Self {
        Self {
            transform_type: TransformType::Homography,
            sip_enabled: true,
            max_rotation: None,
            scale_range: None,
            ..Self::default()
        }
    }
    
    /// Mosaic configuration: allows larger offsets and rotations.
    pub fn mosaic() -> Self {
        Self {
            max_rotation: None,
            scale_range: Some((0.5, 2.0)),
            use_spatial_grid: true,
            ..Self::default()
        }
    }
    
    /// Validate all parameters.
    pub fn validate(&self) { /* single validation method */ }
}
```

### 2. Simple Top-Level Functions

All functions take a `&Config` parameter. Use `&Config::default()` for default settings.

```rust
/// Register two sets of star positions.
///
/// # Example
/// ```
/// // With defaults
/// let result = registration::register(&ref_stars, &target_stars, &Config::default())?;
///
/// // With custom config
/// let config = Config { inlier_threshold: 3.0, ..Config::default() };
/// let result = registration::register(&ref_stars, &target_stars, &config)?;
/// ```
pub fn register(
    ref_stars: &[Star],
    target_stars: &[Star],
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    Registrator::new(config.clone()).register(ref_stars, target_stars)
}

/// Warp an image to align with reference frame.
///
/// # Example
/// ```
/// let result = registration::register(&ref_stars, &target_stars, &Config::default())?;
/// let aligned = registration::warp(&target_image, &result.transform, &Config::default());
/// ```
pub fn warp(image: &AstroImage, transform: &Transform, config: &Config) -> AstroImage {
    warp_to_reference_image(image, transform, config.interpolation)
}

/// Compute transformation from known point correspondences.
///
/// Use when you have manually matched points (e.g., from user marking).
pub fn compute_transform(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform_type: TransformType,
) -> Option<Transform> {
    estimate_transform(ref_points, target_points, transform_type)
}
```

### 3. Simplified Result Struct

```rust
/// Result of image registration.
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    /// Computed transformation (reference → target).
    pub transform: Transform,
    
    /// Optional SIP distortion correction.
    pub sip_correction: Option<SipPolynomial>,
    
    /// Matched star pairs as (ref_index, target_index).
    pub matches: Vec<(usize, usize)>,
    
    /// Per-match residual error in pixels.
    pub residuals: Vec<f64>,
    
    /// Number of inliers (same as matches.len()).
    pub num_inliers: usize,
    
    /// Root-mean-square error in pixels.
    pub rms_error: f64,
    
    /// Maximum residual error in pixels.
    pub max_error: f64,
    
    /// Quality score (0.0-1.0).
    pub quality_score: f64,
    
    /// Processing time in milliseconds.
    pub elapsed_ms: f64,
}
```

### 4. Minimal Public API

```rust
// mod.rs - Only essential re-exports

// Configuration
pub use config::{Config, InterpolationMethod};

// Core types
pub use transform::{Transform, TransformType};

// Results and errors
pub use result::{RegistrationResult, RegistrationError};

// Top-level functions (primary API)
pub use functions::{register, warp, compute_transform};

// Distortion (only if users need manual SIP)
pub use distortion::SipPolynomial;
```

Everything else (`RansacEstimator`, `match_triangles`, `KdTree`, `PointMatch`, etc.) becomes `pub(crate)`.

## Comparison with Industry Standards

| Aspect | Astroalign | OpenCV | Proposed |
|--------|------------|--------|----------|
| Main entry | `register()` | `findHomography()` | `register()` |
| Config params | 3 constants | Function args | Flat struct |
| Transform output | SimilarityTransform | Mat | Transform |
| Warp function | `apply_transform()` | `warpPerspective()` | `warp()` |
| Error handling | Exception | None/Mat | Result |

## Migration Path

### Phase 1: Add New API (Non-Breaking)

1. Create flat `Config` struct alongside existing `RegistrationConfig`
2. Add top-level functions that wrap `Registrator`
3. Add `Config::from_legacy(RegistrationConfig)` conversion
4. Mark old types as `#[deprecated]`

### Phase 2: Internal Migration

1. Update internal code to use new `Config`
2. Update tests to use new API
3. Update examples

### Phase 3: Remove Old API

1. Remove `RegistrationConfig`, `TriangleMatchConfig`, `RansacConfig`, `WarpConfig`, `SipCorrectionConfig`
2. Remove deprecated re-exports
3. Update documentation

## Files to Modify

### New Files
- None (rewrite `config.rs` in place)

### Modified Files
- `config.rs` — Replace nested structs with flat `Config`
- `mod.rs` — Reduce re-exports, add top-level functions
- `pipeline/mod.rs` — Update `Registrator` to use flat `Config`
- `pipeline/result.rs` — Rename `matched_stars` → `matches`
- `ransac/mod.rs` — Take individual params instead of `RansacConfig`
- `triangle/matching.rs` — Take individual params instead of `TriangleMatchConfig`
- `interpolation/mod.rs` — Take individual params instead of `WarpConfig`
- All test files — Update to new API

### External Callers
- `lib.rs` — Update re-exports
- `examples/star_detection.rs` — Use new API
- `examples/full_pipeline.rs` — Use new API
- `examples/plate_solve.rs` — Use new API
- `stacking/weighted/mod.rs` — Use new API
- `testing/` — Update config usage

## Implementation Order

1. **Flatten Config** — Create new flat `Config`, keep old as deprecated
2. **Add Functions** — Add `register()`, `warp()`, etc.
3. **Update Pipeline** — Make `Registrator` use flat config internally
4. **Update Submodules** — RANSAC, triangle, interpolation take individual params
5. **Update Tests** — Migrate all tests to new API
6. **Update Examples** — Migrate examples
7. **Remove Old API** — Delete deprecated types
8. **Update Documentation** — README, doc comments

## Summary

The redesigned API provides:

| Before | After |
|--------|-------|
| 5 config structs | 1 flat Config struct |
| 25+ nested parameters | Same params, flat access |
| Stateful Registrator required | Simple function call |
| 30+ re-exports | ~10 essential types |
| `config.ransac.inlier_threshold` | `config.inlier_threshold` |

**Example comparison:**

```rust
// Before
let config = RegistrationConfig {
    ransac: RansacConfig { inlier_threshold: 3.0, ..Default::default() },
    ..Default::default()
};
let registrator = Registrator::new(config);
let result = registrator.register_stars(&ref_stars, &target_stars)?;
let aligned = warp_to_reference_image(&target, &result.transform, InterpolationMethod::Lanczos3);

// After
let config = Config { inlier_threshold: 3.0, ..Config::default() };
let result = register(&ref_stars, &target_stars, &config)?;
let aligned = warp(&target, &result.transform, &config);
```

## References

- [Astroalign GitHub](https://github.com/quatrope/astroalign) — Minimal API design
- [Astroalign Documentation](https://astroalign.quatrope.org/) — Function signatures
- [OpenCV Geometric Transformations](https://docs.opencv.org/4.x/da/d54/group__imgproc__transform.html) — Function-based API
- [PixInsight StarAlignment](https://www.pixinsight.com/tutorials/sa-distortion/index.html) — Parameter organization
- [image-registration](https://image-registration.readthedocs.io/) — Single entry point pattern
