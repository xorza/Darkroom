# Image Registration Module - Comprehensive Implementation Plan

## Executive Summary

This document outlines a detailed implementation plan for a state-of-the-art astronomical image
registration module. The design synthesizes proven algorithms from professional tools (PixInsight,
Siril, DeepSkyStacker) and academic research (Astroalign, phase correlation methods).

**Target Capabilities:**
- Sub-pixel registration accuracy (< 0.1 pixel RMS)
- Support for translation, similarity, affine, and projective transformations
- Robust handling of rotation, scale changes, and field distortions
- SIMD acceleration for all compute-intensive operations
- GPU-ready architecture for future acceleration

**Algorithm Selection Rationale:**
- **Triangle matching** (Siril/Astroalign approach): Proven, robust, handles rotation/scale naturally
- **Phase correlation**: Fast coarse alignment, GPU-friendly, handles large offsets
- **RANSAC**: Industry standard for robust transformation estimation
- **Lanczos interpolation**: Best quality for sub-pixel resampling

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Registration Pipeline                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐    ┌───────────┐ │
│  │    Input     │───▶│    Coarse    │───▶│     Fine     │───▶│  Transform │ │
│  │   Images     │    │  Alignment   │    │  Alignment   │    │   Apply    │ │
│  └──────────────┘    └──────────────┘    └──────────────┘    └───────────┘ │
│         │                   │                   │                   │        │
│         ▼                   ▼                   ▼                   ▼        │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐    ┌───────────┐ │
│  │    Star      │    │    Phase     │    │   Triangle   │    │  Lanczos  │ │
│  │  Detection   │    │ Correlation  │    │  Matching    │    │  Resample │ │
│  │  (existing)  │    │   (FFT)      │    │  + RANSAC    │    │           │ │
│  └──────────────┘    └──────────────┘    └──────────────┘    └───────────┘ │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Module Structure

```
lumos/src/registration/
├── mod.rs                      # Public API and re-exports
├── IMPLEMENTATION_PLAN.md      # This document
│
├── types.rs                    # Core types and enums
│   ├── Transform               # Transformation enum (Translation, Similarity, Affine, Homography)
│   ├── TransformMatrix         # 3x3 homogeneous transformation matrix
│   ├── RegistrationConfig      # Configuration struct
│   ├── RegistrationResult      # Result with transform, metrics, matched stars
│   └── MatchedStar             # Star correspondence (ref_idx, target_idx, distance)
│
├── star_matching/              # Star pattern matching algorithms
│   ├── mod.rs
│   ├── triangle.rs             # Triangle descriptor and matching
│   ├── geometric_hash.rs       # Geometric hashing for fast lookup
│   ├── voting.rs               # Voting scheme for candidate matches
│   └── tests.rs
│
├── transform/                  # Transformation estimation
│   ├── mod.rs
│   ├── ransac.rs               # RANSAC robust estimation
│   ├── least_squares.rs        # Weighted least squares refinement
│   ├── models.rs               # Translation, Similarity, Affine, Homography
│   └── tests.rs
│
├── phase_correlation/          # FFT-based coarse alignment
│   ├── mod.rs
│   ├── fft.rs                  # FFT wrapper (using rustfft)
│   ├── correlation.rs          # Phase correlation implementation
│   ├── subpixel.rs             # Sub-pixel peak detection
│   ├── simd/                   # SIMD acceleration
│   │   ├── mod.rs
│   │   ├── avx2.rs
│   │   ├── sse.rs
│   │   └── neon.rs
│   └── tests.rs
│
├── interpolation/              # Image resampling
│   ├── mod.rs
│   ├── lanczos.rs              # Lanczos-3 interpolation
│   ├── bicubic.rs              # Bicubic interpolation (faster fallback)
│   ├── bilinear.rs             # Bilinear (fastest, lowest quality)
│   ├── simd/                   # SIMD acceleration
│   │   ├── mod.rs
│   │   ├── avx2.rs
│   │   ├── sse.rs
│   │   └── neon.rs
│   └── tests.rs
│
├── warp.rs                     # Image warping with transforms
│
├── quality.rs                  # Registration quality metrics
│
└── bench.rs                    # Benchmarks (feature-gated)
```

---

## Phase 1: Core Types and Infrastructure

### 1.1 Transformation Types

```rust
/// Supported transformation models with increasing degrees of freedom
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformType {
    /// Translation only (2 DOF: dx, dy)
    Translation,
    /// Translation + Rotation (3 DOF: dx, dy, angle)
    Euclidean,
    /// Translation + Rotation + Uniform Scale (4 DOF)
    Similarity,
    /// Full affine (6 DOF: handles differential scaling and shear)
    Affine,
    /// Projective/Homography (8 DOF: handles perspective)
    Homography,
}

/// 3x3 homogeneous transformation matrix
#[derive(Debug, Clone)]
pub struct TransformMatrix {
    /// Row-major 3x3 matrix [a, b, c, d, e, f, g, h, 1]
    /// For affine: [[a, b, tx], [c, d, ty], [0, 0, 1]]
    /// For homography: [[a, b, c], [d, e, f], [g, h, 1]]
    pub data: [f64; 9],
    pub transform_type: TransformType,
}

impl TransformMatrix {
    /// Create identity transform
    pub fn identity() -> Self;
    
    /// Create translation transform
    pub fn translation(dx: f64, dy: f64) -> Self;
    
    /// Create similarity transform (translation + rotation + scale)
    pub fn similarity(dx: f64, dy: f64, angle: f64, scale: f64) -> Self;
    
    /// Create affine transform from 6 parameters
    pub fn affine(params: [f64; 6]) -> Self;
    
    /// Create homography from 8 parameters (9th is 1.0)
    pub fn homography(params: [f64; 8]) -> Self;
    
    /// Apply transform to a point
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64);
    
    /// Apply inverse transform to a point
    pub fn apply_inverse(&self, x: f64, y: f64) -> (f64, f64);
    
    /// Compute matrix inverse
    pub fn inverse(&self) -> Option<Self>;
    
    /// Compose two transforms: self * other
    pub fn compose(&self, other: &Self) -> Self;
    
    /// Extract translation components
    pub fn translation_components(&self) -> (f64, f64);
    
    /// Extract rotation angle (for similarity/euclidean)
    pub fn rotation_angle(&self) -> f64;
    
    /// Extract scale factor (for similarity)
    pub fn scale_factor(&self) -> f64;
}
```

### 1.2 Configuration and Results

```rust
/// Registration configuration
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Maximum transformation type to consider
    pub transform_type: TransformType,
    
    /// Use phase correlation for coarse alignment first
    pub use_phase_correlation: bool,
    
    /// RANSAC parameters
    pub ransac_iterations: usize,      // Default: 1000
    pub ransac_threshold: f64,         // Inlier threshold in pixels, default: 2.0
    pub ransac_confidence: f64,        // Target confidence, default: 0.999
    
    /// Triangle matching parameters
    pub min_stars_for_matching: usize, // Minimum stars required, default: 10
    pub max_stars_for_matching: usize, // Limit for performance, default: 200
    pub triangle_tolerance: f64,       // Side ratio tolerance, default: 0.01
    
    /// Sub-pixel refinement
    pub refine_with_centroids: bool,   // Use star centroids for refinement
    pub max_refinement_iterations: usize,
    
    /// Quality thresholds
    pub min_matched_stars: usize,      // Minimum matches required, default: 6
    pub max_residual_pixels: f64,      // Maximum acceptable RMS error, default: 1.0
}

/// Result of registration
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    /// Computed transformation matrix
    pub transform: TransformMatrix,
    
    /// Matched star pairs (reference_idx, target_idx)
    pub matched_stars: Vec<(usize, usize)>,
    
    /// Per-match residuals in pixels
    pub residuals: Vec<f64>,
    
    /// RMS registration error in pixels
    pub rms_error: f64,
    
    /// Maximum residual error in pixels
    pub max_error: f64,
    
    /// Number of RANSAC inliers
    pub num_inliers: usize,
    
    /// Registration quality score (0.0 - 1.0)
    pub quality_score: f64,
    
    /// Processing time in milliseconds
    pub elapsed_ms: f64,
}
```

### Tests for Phase 1

```rust
#[cfg(test)]
mod tests {
    // Transform matrix tests
    #[test] fn test_identity_transform();
    #[test] fn test_translation_transform();
    #[test] fn test_rotation_90_degrees();
    #[test] fn test_scale_transform();
    #[test] fn test_similarity_composition();
    #[test] fn test_affine_inverse();
    #[test] fn test_homography_inverse();
    #[test] fn test_transform_point_roundtrip();
    #[test] fn test_compose_translations();
    #[test] fn test_compose_rotations();
    
    // Config validation tests
    #[test] fn test_config_default_values();
    #[test] fn test_config_validation();
}
```

---

## Phase 2: Triangle Matching Algorithm

### 2.1 Algorithm Description

The triangle matching algorithm identifies similar star patterns between images:

1. **Triangle Formation**: Select N brightest stars, form all possible triangles
2. **Triangle Descriptor**: Compute invariant features (side ratios, angles)
3. **Hash Table Lookup**: Use geometric hashing for O(1) candidate retrieval
4. **Voting**: Accumulate votes for consistent star correspondences
5. **Verification**: Validate top candidates with geometric consistency

### 2.2 Triangle Descriptor

```rust
/// Triangle formed from three stars, with invariant descriptors
#[derive(Debug, Clone)]
pub struct Triangle {
    /// Indices of the three stars (sorted by some criterion)
    pub star_indices: [usize; 3],
    
    /// Side lengths sorted: a <= b <= c
    pub sides: [f64; 3],
    
    /// Invariant ratios: (a/c, b/c) - independent of scale
    pub ratios: (f64, f64),
    
    /// Interior angles in radians
    pub angles: [f64; 3],
    
    /// Orientation (clockwise or counter-clockwise)
    pub orientation: Orientation,
}

impl Triangle {
    /// Create triangle from three star positions
    pub fn from_positions(
        idx: [usize; 3],
        positions: [(f64, f64); 3],
    ) -> Self;
    
    /// Check if two triangles are similar within tolerance
    pub fn is_similar(&self, other: &Triangle, tolerance: f64) -> bool;
    
    /// Compute hash key for geometric hashing
    pub fn hash_key(&self, bins: usize) -> (usize, usize);
}

/// Geometric hash table for fast triangle lookup
pub struct TriangleHashTable {
    /// Hash table: bins × bins grid of triangle indices
    table: Vec<Vec<usize>>,
    bins: usize,
}

impl TriangleHashTable {
    /// Build hash table from triangles
    pub fn build(triangles: &[Triangle], bins: usize) -> Self;
    
    /// Find candidate matches for a query triangle
    pub fn find_candidates(&self, query: &Triangle) -> Vec<usize>;
}
```

### 2.3 Star Matching

```rust
/// Match stars between reference and target images using triangle matching
pub fn match_stars_triangles(
    ref_stars: &[Star],
    target_stars: &[Star],
    config: &TriangleMatchConfig,
) -> Vec<StarMatch>;

/// Configuration for triangle matching
#[derive(Debug, Clone)]
pub struct TriangleMatchConfig {
    /// Maximum number of stars to use (brightest N)
    pub max_stars: usize,           // Default: 50
    
    /// Tolerance for side ratio comparison
    pub ratio_tolerance: f64,       // Default: 0.01
    
    /// Minimum votes required to consider a match
    pub min_votes: usize,           // Default: 3
    
    /// Use orientation check (handles mirrored images)
    pub check_orientation: bool,    // Default: true
}

/// A matched star pair with confidence
#[derive(Debug, Clone)]
pub struct StarMatch {
    pub ref_idx: usize,
    pub target_idx: usize,
    pub votes: usize,
    pub confidence: f64,
}
```

### 2.4 Implementation Details

**Triangle Selection Strategy:**
- Use N brightest stars (default N=50, configurable)
- Form triangles from all combinations: C(N,3) triangles
- For N=50: 19,600 triangles per image
- Filter degenerate triangles (very small area, collinear points)

**Geometric Hashing:**
- Quantize (a/c, b/c) ratios into bins × bins grid
- Default: 100 × 100 = 10,000 bins
- Each bin stores list of triangle indices
- Query: find all triangles in same bin as query

**Voting Scheme:**
- For each matching triangle pair, vote for the 3 star correspondences
- Count votes for each (ref_star, target_star) pair
- Filter pairs with votes >= min_votes
- Resolve conflicts (one-to-many mappings) by taking highest vote

### Tests for Phase 2

```rust
#[cfg(test)]
mod tests {
    // Triangle descriptor tests
    #[test] fn test_triangle_from_positions();
    #[test] fn test_triangle_ratios_scale_invariant();
    #[test] fn test_triangle_similarity_check();
    #[test] fn test_triangle_orientation();
    #[test] fn test_degenerate_triangle_detection();
    
    // Hash table tests
    #[test] fn test_hash_table_build();
    #[test] fn test_hash_table_lookup_exact();
    #[test] fn test_hash_table_lookup_similar();
    #[test] fn test_hash_table_empty();
    
    // Matching tests
    #[test] fn test_match_identical_star_lists();
    #[test] fn test_match_translated_stars();
    #[test] fn test_match_rotated_stars();
    #[test] fn test_match_scaled_stars();
    #[test] fn test_match_with_missing_stars();
    #[test] fn test_match_with_extra_stars();
    #[test] fn test_match_subset_detection();
    #[test] fn test_match_mirrored_image();
    
    // Edge cases
    #[test] fn test_too_few_stars();
    #[test] fn test_all_collinear_stars();
    #[test] fn test_duplicate_positions();
}
```

### Benchmarks for Phase 2

```rust
#[cfg(feature = "bench")]
mod bench {
    // Triangle formation benchmarks
    fn bench_triangle_formation_50_stars(c: &mut Criterion);
    fn bench_triangle_formation_100_stars(c: &mut Criterion);
    fn bench_triangle_formation_200_stars(c: &mut Criterion);
    
    // Hash table benchmarks
    fn bench_hash_table_build_1000_triangles(c: &mut Criterion);
    fn bench_hash_table_lookup(c: &mut Criterion);
    
    // Full matching benchmarks
    fn bench_match_50_stars_translated(c: &mut Criterion);
    fn bench_match_100_stars_rotated(c: &mut Criterion);
    fn bench_match_with_outliers(c: &mut Criterion);
}
```

---

## Phase 3: RANSAC Transformation Estimation

### 3.1 Algorithm Description

RANSAC (Random Sample Consensus) robustly estimates transformations in the presence of outliers:

1. **Random Sample**: Select minimum points needed for transform (4 for homography)
2. **Model Estimation**: Compute candidate transformation from sample
3. **Consensus**: Count inliers (points within threshold of model)
4. **Iteration**: Repeat, keeping best model
5. **Refinement**: Final least-squares fit on all inliers

### 3.2 Implementation

```rust
/// RANSAC estimator for robust transformation fitting
pub struct RansacEstimator {
    config: RansacConfig,
}

#[derive(Debug, Clone)]
pub struct RansacConfig {
    /// Maximum iterations
    pub max_iterations: usize,      // Default: 1000
    
    /// Inlier distance threshold in pixels
    pub inlier_threshold: f64,      // Default: 2.0
    
    /// Early termination confidence
    pub confidence: f64,            // Default: 0.999
    
    /// Minimum inlier ratio to accept model
    pub min_inlier_ratio: f64,      // Default: 0.5
}

impl RansacEstimator {
    /// Estimate transformation from matched point pairs
    pub fn estimate(
        &self,
        ref_points: &[(f64, f64)],
        target_points: &[(f64, f64)],
        transform_type: TransformType,
    ) -> Option<RansacResult>;
}

#[derive(Debug, Clone)]
pub struct RansacResult {
    /// Best transformation found
    pub transform: TransformMatrix,
    
    /// Indices of inlier matches
    pub inliers: Vec<usize>,
    
    /// Number of iterations performed
    pub iterations: usize,
}

/// Minimum samples needed for each transform type
fn min_samples(transform_type: TransformType) -> usize {
    match transform_type {
        TransformType::Translation => 1,
        TransformType::Euclidean => 2,
        TransformType::Similarity => 2,
        TransformType::Affine => 3,
        TransformType::Homography => 4,
    }
}
```

### 3.3 Adaptive Iteration Count

```rust
/// Compute number of iterations needed for given confidence
fn adaptive_iterations(
    inlier_ratio: f64,
    sample_size: usize,
    confidence: f64,
) -> usize {
    // N = log(1 - confidence) / log(1 - w^n)
    // where w = inlier_ratio, n = sample_size
    let w_n = inlier_ratio.powi(sample_size as i32);
    let log_conf = (1.0 - confidence).ln();
    let log_outlier = (1.0 - w_n).ln();
    
    (log_conf / log_outlier).ceil() as usize
}
```

### 3.4 Least Squares Refinement

```rust
/// Refine transformation using weighted least squares on inliers
pub fn refine_transform(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    inliers: &[usize],
    transform_type: TransformType,
) -> TransformMatrix;

/// Solve for affine parameters using SVD
fn solve_affine_svd(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
) -> [f64; 6];

/// Solve for homography using DLT (Direct Linear Transform)
fn solve_homography_dlt(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
) -> [f64; 8];
```

### Tests for Phase 3

```rust
#[cfg(test)]
mod tests {
    // RANSAC basic tests
    #[test] fn test_ransac_perfect_translation();
    #[test] fn test_ransac_perfect_rotation();
    #[test] fn test_ransac_perfect_affine();
    #[test] fn test_ransac_perfect_homography();
    
    // Outlier rejection tests
    #[test] fn test_ransac_with_10_percent_outliers();
    #[test] fn test_ransac_with_30_percent_outliers();
    #[test] fn test_ransac_with_50_percent_outliers();
    
    // Edge cases
    #[test] fn test_ransac_insufficient_points();
    #[test] fn test_ransac_all_outliers();
    #[test] fn test_ransac_collinear_points();
    
    // Least squares tests
    #[test] fn test_ls_refine_improves_accuracy();
    #[test] fn test_ls_affine_overdetermined();
    #[test] fn test_ls_homography_overdetermined();
    
    // Adaptive iteration tests
    #[test] fn test_adaptive_iterations_high_inlier_ratio();
    #[test] fn test_adaptive_iterations_low_inlier_ratio();
    #[test] fn test_early_termination();
}
```

### Benchmarks for Phase 3

```rust
#[cfg(feature = "bench")]
mod bench {
    fn bench_ransac_100_points_10pct_outliers(c: &mut Criterion);
    fn bench_ransac_100_points_30pct_outliers(c: &mut Criterion);
    fn bench_ransac_affine_vs_homography(c: &mut Criterion);
    fn bench_least_squares_refinement(c: &mut Criterion);
}
```

---

## Phase 4: Phase Correlation (Coarse Alignment)

### 4.1 Algorithm Description

Phase correlation uses FFT to find translation between images:

1. **FFT**: Compute 2D FFT of both images
2. **Cross-Power Spectrum**: Multiply FFT₁ by conjugate of FFT₂, normalize
3. **Inverse FFT**: Peak location gives translation offset
4. **Sub-pixel Refinement**: Parabolic fit around peak for sub-pixel accuracy

### 4.2 Implementation

```rust
/// Phase correlation for coarse translation estimation
pub struct PhaseCorrelator {
    /// Cached FFT planner for efficiency
    planner: Arc<dyn FftPlanner<f32>>,
    
    /// Configuration
    config: PhaseCorrelationConfig,
}

#[derive(Debug, Clone)]
pub struct PhaseCorrelationConfig {
    /// Apply Hann window to reduce edge effects
    pub use_windowing: bool,        // Default: true
    
    /// Sub-pixel interpolation method
    pub subpixel_method: SubpixelMethod,
    
    /// Minimum correlation peak to accept
    pub min_peak_value: f32,        // Default: 0.1
}

#[derive(Debug, Clone, Copy)]
pub enum SubpixelMethod {
    /// No sub-pixel refinement
    None,
    /// Parabolic fit (fast, ~0.1 pixel accuracy)
    Parabolic,
    /// Gaussian fit (slower, ~0.05 pixel accuracy)
    Gaussian,
    /// Centroid (robust to noise)
    Centroid,
}

impl PhaseCorrelator {
    /// Estimate translation between two images
    pub fn correlate(
        &self,
        reference: &[f32],
        target: &[f32],
        width: usize,
        height: usize,
    ) -> PhaseCorrelationResult;
}

#[derive(Debug, Clone)]
pub struct PhaseCorrelationResult {
    /// Estimated translation (dx, dy)
    pub translation: (f64, f64),
    
    /// Peak correlation value (0.0 - 1.0)
    pub peak_value: f64,
    
    /// Secondary peak ratio (lower = more confident)
    pub secondary_peak_ratio: f64,
}
```

### 4.3 Rotation and Scale via Log-Polar Transform

For handling rotation and scale, use log-polar representation:

```rust
/// Extended phase correlation with rotation/scale detection
pub fn correlate_with_rotation_scale(
    reference: &[f32],
    target: &[f32],
    width: usize,
    height: usize,
) -> RotationScaleResult;

#[derive(Debug, Clone)]
pub struct RotationScaleResult {
    pub translation: (f64, f64),
    pub rotation: f64,      // Radians
    pub scale: f64,         // Scale factor
    pub confidence: f64,
}

/// Convert image to log-polar representation
fn to_log_polar(
    image: &[f32],
    width: usize,
    height: usize,
    angular_bins: usize,
    radial_bins: usize,
) -> Vec<f32>;
```

### 4.4 SIMD Optimization

```rust
/// SIMD-accelerated complex multiply for cross-power spectrum
pub mod simd {
    /// AVX2 complex multiply: (a + bi) * (c - di) = (ac + bd) + (bc - ad)i
    #[cfg(target_arch = "x86_64")]
    pub fn complex_multiply_conjugate_avx2(
        fft1_re: &[f32], fft1_im: &[f32],
        fft2_re: &[f32], fft2_im: &[f32],
        out_re: &mut [f32], out_im: &mut [f32],
    );
    
    /// NEON implementation for ARM
    #[cfg(target_arch = "aarch64")]
    pub fn complex_multiply_conjugate_neon(...);
    
    /// SSE4.1 fallback
    #[cfg(target_arch = "x86_64")]
    pub fn complex_multiply_conjugate_sse41(...);
}
```

### Tests for Phase 4

```rust
#[cfg(test)]
mod tests {
    // Basic correlation tests
    #[test] fn test_correlate_identical_images();
    #[test] fn test_correlate_translated_1_pixel();
    #[test] fn test_correlate_translated_10_pixels();
    #[test] fn test_correlate_translated_subpixel();
    #[test] fn test_correlate_negative_translation();
    
    // Sub-pixel accuracy tests
    #[test] fn test_subpixel_parabolic_accuracy();
    #[test] fn test_subpixel_gaussian_accuracy();
    #[test] fn test_subpixel_centroid_accuracy();
    
    // Robustness tests
    #[test] fn test_correlate_with_noise();
    #[test] fn test_correlate_partial_overlap();
    #[test] fn test_correlate_different_brightness();
    
    // Rotation/scale tests
    #[test] fn test_detect_rotation_45_degrees();
    #[test] fn test_detect_scale_1_5x();
    #[test] fn test_detect_rotation_and_scale();
    
    // SIMD tests
    #[test] fn test_simd_matches_scalar();
    #[test] fn test_simd_complex_multiply();
}
```

### Benchmarks for Phase 4

```rust
#[cfg(feature = "bench")]
mod bench {
    fn bench_phase_correlation_512x512(c: &mut Criterion);
    fn bench_phase_correlation_1024x1024(c: &mut Criterion);
    fn bench_phase_correlation_2048x2048(c: &mut Criterion);
    fn bench_phase_correlation_4096x4096(c: &mut Criterion);
    
    fn bench_log_polar_transform(c: &mut Criterion);
    fn bench_subpixel_refinement(c: &mut Criterion);
    
    fn bench_simd_complex_multiply(c: &mut Criterion);
}
```

---

## Phase 5: Image Interpolation

### 5.1 Interpolation Methods

```rust
/// Interpolation method for image resampling
#[derive(Debug, Clone, Copy)]
pub enum InterpolationMethod {
    /// Nearest neighbor (fastest, blocky)
    Nearest,
    /// Bilinear (fast, smooth)
    Bilinear,
    /// Bicubic (good quality, moderate speed)
    Bicubic,
    /// Lanczos-3 (best quality, slowest)
    Lanczos3,
    /// Lanczos-4 (slightly better than Lanczos-3)
    Lanczos4,
}
```

### 5.2 Lanczos Kernel

```rust
/// Lanczos interpolation kernel
pub struct LanczosKernel {
    /// Kernel size (3 or 4 typically)
    a: usize,
    /// Precomputed weights table for sub-pixel offsets
    weights: Vec<[f32; 8]>,  // For a=3: 6 weights per sample
}

impl LanczosKernel {
    /// Create Lanczos-a kernel
    pub fn new(a: usize) -> Self;
    
    /// Compute kernel weight
    #[inline]
    fn lanczos(x: f32, a: f32) -> f32 {
        if x == 0.0 {
            1.0
        } else if x.abs() < a {
            let pi_x = std::f32::consts::PI * x;
            let pi_x_a = pi_x / a;
            (pi_x.sin() / pi_x) * (pi_x_a.sin() / pi_x_a)
        } else {
            0.0
        }
    }
    
    /// Interpolate at sub-pixel position
    pub fn interpolate(
        &self,
        image: &[f32],
        width: usize,
        height: usize,
        x: f32,
        y: f32,
    ) -> f32;
}
```

### 5.3 SIMD-Accelerated Interpolation

```rust
pub mod simd {
    /// Process 8 output pixels in parallel using AVX2
    #[cfg(target_arch = "x86_64")]
    pub fn lanczos_row_avx2(
        src: &[f32],
        dst: &mut [f32],
        src_width: usize,
        x_offsets: &[f32],  // Sub-pixel x positions
        kernel: &LanczosKernel,
    );
    
    /// Bilinear interpolation for 8 pixels
    #[cfg(target_arch = "x86_64")]
    pub fn bilinear_8pixels_avx2(
        src: &[f32],
        width: usize,
        x: &[f32; 8],
        y: &[f32; 8],
    ) -> [f32; 8];
}
```

### 5.4 Image Warping

```rust
/// Warp image using transformation matrix
pub fn warp_image(
    src: &[f32],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
    transform: &TransformMatrix,
    method: InterpolationMethod,
) -> Vec<f32>;

/// Warp image with parallel processing
pub fn warp_image_parallel(
    src: &[f32],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
    transform: &TransformMatrix,
    method: InterpolationMethod,
) -> Vec<f32>;
```

### Tests for Phase 5

```rust
#[cfg(test)]
mod tests {
    // Kernel tests
    #[test] fn test_lanczos_kernel_sum_to_one();
    #[test] fn test_lanczos_kernel_symmetry();
    #[test] fn test_lanczos_at_integer_positions();
    
    // Interpolation accuracy tests
    #[test] fn test_bilinear_gradient();
    #[test] fn test_bicubic_smooth_curve();
    #[test] fn test_lanczos_sinc_preservation();
    #[test] fn test_interpolation_at_integer_is_exact();
    
    // Edge handling tests
    #[test] fn test_interpolation_near_edge();
    #[test] fn test_interpolation_outside_bounds();
    
    // Warp tests
    #[test] fn test_warp_identity();
    #[test] fn test_warp_translation();
    #[test] fn test_warp_rotation_90();
    #[test] fn test_warp_scale_2x();
    #[test] fn test_warp_affine();
    #[test] fn test_warp_homography();
    
    // SIMD tests
    #[test] fn test_simd_bilinear_matches_scalar();
    #[test] fn test_simd_lanczos_matches_scalar();
}
```

### Benchmarks for Phase 5

```rust
#[cfg(feature = "bench")]
mod bench {
    fn bench_bilinear_1024x1024(c: &mut Criterion);
    fn bench_bicubic_1024x1024(c: &mut Criterion);
    fn bench_lanczos3_1024x1024(c: &mut Criterion);
    fn bench_lanczos4_1024x1024(c: &mut Criterion);
    
    fn bench_warp_translation(c: &mut Criterion);
    fn bench_warp_rotation(c: &mut Criterion);
    fn bench_warp_affine(c: &mut Criterion);
    fn bench_warp_homography(c: &mut Criterion);
    
    fn bench_simd_vs_scalar(c: &mut Criterion);
}
```

---

## Phase 6: Full Pipeline Integration

### 6.1 High-Level API

```rust
/// Main registration function
pub fn register_images(
    reference: &AstroImage,
    target: &AstroImage,
    config: &RegistrationConfig,
) -> Result<RegistrationResult, RegistrationError>;

/// Register and warp target image to match reference
pub fn register_and_warp(
    reference: &AstroImage,
    target: &AstroImage,
    config: &RegistrationConfig,
    interpolation: InterpolationMethod,
) -> Result<(AstroImage, RegistrationResult), RegistrationError>;

/// Register multiple images to a reference
pub fn register_stack(
    reference: &AstroImage,
    targets: &[AstroImage],
    config: &RegistrationConfig,
) -> Vec<Result<RegistrationResult, RegistrationError>>;
```

### 6.2 Builder Pattern

```rust
pub struct RegistrationConfigBuilder {
    config: RegistrationConfig,
}

impl RegistrationConfigBuilder {
    pub fn new() -> Self;
    
    // Transform type
    pub fn translation_only(mut self) -> Self;
    pub fn with_rotation(mut self) -> Self;
    pub fn with_scale(mut self) -> Self;
    pub fn full_affine(mut self) -> Self;
    pub fn full_homography(mut self) -> Self;
    
    // Coarse alignment
    pub fn use_phase_correlation(mut self, enable: bool) -> Self;
    
    // RANSAC parameters
    pub fn ransac_iterations(mut self, n: usize) -> Self;
    pub fn ransac_threshold(mut self, pixels: f64) -> Self;
    
    // Star matching
    pub fn max_stars(mut self, n: usize) -> Self;
    pub fn triangle_tolerance(mut self, tol: f64) -> Self;
    
    // Quality
    pub fn min_matched_stars(mut self, n: usize) -> Self;
    pub fn max_residual(mut self, pixels: f64) -> Self;
    
    pub fn build(self) -> RegistrationConfig;
}
```

### 6.3 Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("Insufficient stars detected: found {found}, need {required}")]
    InsufficientStars { found: usize, required: usize },
    
    #[error("No matching star patterns found between images")]
    NoMatchingPatterns,
    
    #[error("RANSAC failed to find valid transformation")]
    RansacFailed,
    
    #[error("Registration accuracy too low: {rms_error:.3} pixels (max: {max_allowed:.3})")]
    AccuracyTooLow { rms_error: f64, max_allowed: f64 },
    
    #[error("Images have incompatible dimensions")]
    DimensionMismatch,
    
    #[error("Star detection failed: {0}")]
    StarDetection(String),
}
```

### Tests for Phase 6

```rust
#[cfg(test)]
mod tests {
    // End-to-end tests
    #[test] fn test_register_translated_synthetic();
    #[test] fn test_register_rotated_synthetic();
    #[test] fn test_register_scaled_synthetic();
    #[test] fn test_register_affine_synthetic();
    
    // Real image tests (ignored by default)
    #[test] #[ignore] fn test_register_real_dslr_images();
    #[test] #[ignore] fn test_register_telescope_images();
    
    // Error handling tests
    #[test] fn test_error_insufficient_stars();
    #[test] fn test_error_no_overlap();
    #[test] fn test_error_dimension_mismatch();
    
    // Builder tests
    #[test] fn test_builder_default();
    #[test] fn test_builder_presets();
}
```

### Benchmarks for Phase 6

```rust
#[cfg(feature = "bench")]
mod bench {
    // Full pipeline benchmarks
    fn bench_register_1024x1024_50_stars(c: &mut Criterion);
    fn bench_register_2048x2048_100_stars(c: &mut Criterion);
    fn bench_register_4096x4096_200_stars(c: &mut Criterion);
    
    // Component breakdown
    fn bench_pipeline_star_detection(c: &mut Criterion);
    fn bench_pipeline_triangle_matching(c: &mut Criterion);
    fn bench_pipeline_ransac(c: &mut Criterion);
    fn bench_pipeline_warp(c: &mut Criterion);
}
```

---

## Phase 7: Quality Metrics and Validation

### 7.1 Quality Metrics

```rust
/// Registration quality assessment
pub struct RegistrationQuality {
    /// RMS error of matched star positions after transformation
    pub rms_error: f64,
    
    /// Maximum residual error
    pub max_error: f64,
    
    /// Percentage of stars successfully matched
    pub match_ratio: f64,
    
    /// RANSAC inlier ratio
    pub inlier_ratio: f64,
    
    /// Condition number of transformation matrix (stability indicator)
    pub condition_number: f64,
    
    /// Estimated sub-pixel accuracy
    pub estimated_accuracy: f64,
}

/// Compute quality metrics for registration result
pub fn assess_quality(
    result: &RegistrationResult,
    ref_stars: &[Star],
    target_stars: &[Star],
) -> RegistrationQuality;
```

### 7.2 Validation Against Known Transformations

```rust
/// Validate registration against ground truth
pub fn validate_registration(
    result: &RegistrationResult,
    ground_truth: &TransformMatrix,
) -> ValidationResult;

#[derive(Debug)]
pub struct ValidationResult {
    /// Translation error in pixels
    pub translation_error: f64,
    
    /// Rotation error in degrees
    pub rotation_error: f64,
    
    /// Scale error as ratio (1.0 = perfect)
    pub scale_error: f64,
    
    /// Overall transformation error (Frobenius norm)
    pub matrix_error: f64,
}
```

### 7.3 Visual Debugging

```rust
/// Generate debug visualization
pub fn visualize_registration(
    reference: &AstroImage,
    target: &AstroImage,
    result: &RegistrationResult,
    output_path: &Path,
) -> std::io::Result<()>;

/// Overlay matched stars
pub fn visualize_matches(
    image: &AstroImage,
    ref_stars: &[Star],
    target_stars: &[Star],
    matches: &[(usize, usize)],
    output_path: &Path,
) -> std::io::Result<()>;
```

---

## Implementation Schedule

### Week 1-2: Phase 1 + Phase 2
- Core types and transformation matrices
- Triangle formation and descriptor computation
- Geometric hashing and matching
- Unit tests for all components

### Week 3: Phase 3
- RANSAC implementation
- Least squares refinement
- Adaptive iteration count
- Robustness testing with synthetic outliers

### Week 4: Phase 4
- FFT integration (rustfft crate)
- Phase correlation implementation
- Sub-pixel peak detection
- SIMD optimization for complex multiply

### Week 5: Phase 5
- Lanczos kernel implementation
- Bilinear/bicubic fallbacks
- Image warping with transforms
- SIMD optimization for interpolation

### Week 6: Phase 6 + Phase 7
- Full pipeline integration
- Builder pattern API
- Quality metrics
- End-to-end testing
- Performance optimization

---

## Dependencies

```toml
[dependencies]
# FFT for phase correlation
rustfft = "6.1"

# Linear algebra for transformation solving
nalgebra = "0.32"

# Parallel processing
rayon = "1.8"

# Error handling
thiserror = "1.0"

# Random number generation for RANSAC
rand = "0.8"
rand_chacha = "0.3"  # Deterministic RNG for reproducible tests

[dev-dependencies]
# Benchmarking
criterion = "0.5"

# Property-based testing
proptest = "1.0"
```

---

## Success Criteria

| Metric | Target | Stretch Goal |
|--------|--------|--------------|
| Sub-pixel accuracy | < 0.1 pixel RMS | < 0.05 pixel RMS |
| Matching success rate | > 95% | > 99% |
| Processing time (2K×2K) | < 500ms | < 100ms |
| Processing time (4K×4K) | < 2s | < 500ms |
| Outlier tolerance | 30% | 50% |
| Rotation detection | ±180° | Any angle |
| Scale detection | 0.5x - 2.0x | 0.1x - 10x |
| Memory usage | < 4x image size | < 2x image size |

---

## References

### Academic Papers
1. Beroiz, M., Cabral, J. B., & Sanchez, B. (2020). Astroalign: A Python module for 
   astronomical image registration. Astronomy and Computing, 32, 100384.
2. Stetson, P. B. (1987). DAOFIND: Stellar photometry. PASP, 99, 191.
3. Fischler, M. A., & Bolles, R. C. (1981). Random sample consensus: a paradigm for 
   model fitting. Communications of the ACM, 24(6), 381-395.

### Software References
- [Siril Registration Documentation](https://siril.readthedocs.io/en/latest/preprocessing/registration.html)
- [PixInsight StarAlignment](https://www.pixinsight.com/tutorials/sa-distortion/)
- [DeepSkyStacker Technical Info](http://deepskystacker.free.fr/english/technical.htm)
- [Astroalign Python Package](https://astroalign.quatrope.org/)

### Rust Crates
- [rustfft](https://crates.io/crates/rustfft) - FFT implementation
- [nalgebra](https://crates.io/crates/nalgebra) - Linear algebra
- [image](https://crates.io/crates/image) - Image loading/saving
