# Image Registration Module - Comprehensive Implementation Plan

## Executive Summary

This document outlines a detailed implementation plan for a state-of-the-art astronomical image
registration module. The design synthesizes proven algorithms from professional tools (PixInsight,
Siril, DeepSkyStacker) and academic research (Astroalign, phase correlation methods).

**Target Capabilities:**
- Sub-pixel registration accuracy (< 0.1 pixel RMS)
- Support for translation, similarity, affine, and projective transformations
- Robust handling of rotation, scale changes, and field distortions
- SIMD acceleration for all compute-intensive operations (SSE4.1, AVX2, NEON)
- GPU-ready architecture for future acceleration

**Algorithm Selection Rationale:**
- **Triangle matching** (Siril/Astroalign approach): Proven, robust, handles rotation/scale naturally
- **Phase correlation**: Fast coarse alignment, GPU-friendly, handles large offsets
- **RANSAC**: Industry standard for robust transformation estimation
- **Lanczos interpolation**: Best quality for sub-pixel resampling

---

## Architecture Overview

```
+-----------------------------------------------------------------------------+
|                         Registration Pipeline                                |
+-----------------------------------------------------------------------------+
|                                                                              |
|  +------------+    +------------+    +------------+    +-----------+        |
|  |    Input   |--->|   Coarse   |--->|    Fine    |--->| Transform |        |
|  |   Images   |    | Alignment  |    | Alignment  |    |   Apply   |        |
|  +------------+    +------------+    +------------+    +-----------+        |
|         |                |                 |                 |               |
|         v                v                 v                 v               |
|  +------------+    +------------+    +------------+    +-----------+        |
|  |    Star    |    |   Phase    |    |  Triangle  |    |  Lanczos  |        |
|  | Detection  |    |Correlation |    |  Matching  |    |  Resample |        |
|  | (existing) |    |   (FFT)    |    |  + RANSAC  |    |           |        |
|  +------------+    +------------+    +------------+    +-----------+        |
|                                                                              |
+-----------------------------------------------------------------------------+
```

---

## Module Structure

```
lumos/src/registration/
|-- mod.rs                      # Public API and re-exports
|-- IMPLEMENTATION_PLAN.md      # This document
|
|-- types/                      # Core types and configuration
|   |-- mod.rs                  # TransformType, TransformMatrix, Config, Result
|   |-- tests.rs                # Unit tests for types
|   +-- bench.rs                # Benchmarks (matrix ops, composition)
|
|-- triangle/                   # Triangle matching algorithm
|   |-- mod.rs                  # Triangle descriptor and hash table
|   |-- matching.rs             # Star matching via triangle voting
|   |-- simd/                   # SIMD-accelerated distance calculations
|   |   |-- mod.rs
|   |   |-- sse.rs              # SSE4.1 implementation
|   |   +-- neon.rs             # ARM NEON implementation
|   |-- tests.rs                # Comprehensive matching tests
|   +-- bench.rs                # Triangle formation, hash table, matching benchmarks
|
|-- ransac/                     # RANSAC transformation estimation
|   |-- mod.rs                  # RansacEstimator, RansacConfig, RansacResult
|   |-- estimators.rs           # Transform-specific estimators (translation, affine, etc.)
|   |-- refinement.rs           # Least-squares refinement
|   |-- simd/                   # SIMD-accelerated residual calculations
|   |   |-- mod.rs
|   |   |-- sse.rs
|   |   +-- neon.rs
|   |-- tests.rs                # Outlier rejection, refinement tests
|   +-- bench.rs                # RANSAC iteration benchmarks
|
|-- phase_correlation/          # FFT-based coarse alignment
|   |-- mod.rs                  # PhaseCorrelator, config, result types
|   |-- fft_ops.rs              # FFT operations using rustfft
|   |-- subpixel.rs             # Sub-pixel peak detection methods
|   |-- simd/                   # SIMD-accelerated complex arithmetic
|   |   |-- mod.rs
|   |   |-- sse.rs              # SSE4.1 complex multiply
|   |   +-- neon.rs             # NEON complex multiply
|   |-- tests.rs                # Correlation accuracy tests
|   +-- bench.rs                # FFT, correlation benchmarks by image size
|
|-- interpolation/              # Image resampling
|   |-- mod.rs                  # InterpolationMethod, public API
|   |-- lanczos.rs              # Lanczos-3/4 kernel and interpolation
|   |-- bicubic.rs              # Bicubic interpolation
|   |-- bilinear.rs             # Bilinear interpolation
|   |-- nearest.rs              # Nearest neighbor
|   |-- simd/                   # SIMD-accelerated interpolation
|   |   |-- mod.rs
|   |   |-- sse.rs              # SSE4.1 row processing
|   |   +-- neon.rs             # NEON row processing
|   |-- tests.rs                # Kernel properties, accuracy tests
|   +-- bench.rs                # Interpolation method comparison benchmarks
|
|-- pipeline/                   # Full registration pipeline
|   |-- mod.rs                  # Registrator, register_images, register_and_warp
|   |-- builder.rs              # RegistrationConfigBuilder
|   |-- tests.rs                # End-to-end pipeline tests
|   +-- bench.rs                # Full pipeline benchmarks
|
+-- quality/                    # Registration quality metrics
    |-- mod.rs                  # QualityMetrics, assess_quality
    |-- validation.rs           # Validation against ground truth
    |-- tests.rs                # Quality assessment tests
    +-- bench.rs                # Quality computation benchmarks
```

---

## Phase 1: Core Types and Infrastructure

### 1.1 Transformation Types

```rust
/// Supported transformation models with increasing degrees of freedom
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn identity() -> Self;
    pub fn from_translation(dx: f64, dy: f64) -> Self;
    pub fn from_scale(sx: f64, sy: f64) -> Self;
    pub fn from_rotation(angle: f64) -> Self;
    pub fn from_rotation_around(angle: f64, cx: f64, cy: f64) -> Self;
    pub fn similarity(dx: f64, dy: f64, angle: f64, scale: f64) -> Self;
    pub fn affine(params: [f64; 6]) -> Self;
    pub fn homography(params: [f64; 8]) -> Self;
    
    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64);
    pub fn inverse(&self) -> Self;  // Panics on singular matrix
    pub fn compose(&self, other: &Self) -> Self;
    
    pub fn translation_components(&self) -> (f64, f64);
    pub fn rotation_angle(&self) -> f64;
    pub fn scale_factor(&self) -> f64;
    pub fn is_valid(&self) -> bool;
}
```

### 1.2 Configuration and Results

```rust
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    pub transform_type: TransformType,
    pub use_phase_correlation: bool,
    
    // RANSAC parameters
    pub ransac_iterations: usize,      // Default: 1000
    pub ransac_threshold: f64,         // Default: 2.0 pixels
    pub ransac_confidence: f64,        // Default: 0.999
    
    // Triangle matching parameters
    pub min_stars_for_matching: usize, // Default: 10
    pub max_stars_for_matching: usize, // Default: 200
    pub triangle_tolerance: f64,       // Default: 0.01
    
    // Quality thresholds
    pub min_matched_stars: usize,      // Default: 6
    pub max_residual_pixels: f64,      // Default: 1.0
}

#[derive(Debug, Clone)]
pub struct RegistrationResult {
    pub transform: TransformMatrix,
    pub matched_stars: Vec<StarMatch>,
    pub residuals: Vec<f64>,
    pub rms_error: f64,
    pub max_error: f64,
    pub num_inliers: usize,
    pub quality_score: f64,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone)]
pub struct StarMatch {
    pub ref_idx: usize,
    pub target_idx: usize,
    pub ref_pos: (f64, f64),
    pub target_pos: (f64, f64),
}
```

### types/tests.rs

```rust
#[cfg(test)]
mod tests {
    // Matrix construction
    fn test_identity_transform();
    fn test_translation_transform();
    fn test_rotation_90_degrees();
    fn test_rotation_180_degrees();
    fn test_rotation_angle_extraction();
    fn test_scale_transform();
    fn test_scale_factor_extraction();
    
    // Matrix operations
    fn test_transform_point();
    fn test_transform_point_roundtrip();
    fn test_inverse_translation();
    fn test_inverse_rotation();
    fn test_inverse_affine();
    fn test_inverse_homography();
    fn test_compose_translations();
    fn test_compose_rotations();
    fn test_compose_mixed();
    
    // Validation
    fn test_is_valid_identity();
    fn test_is_valid_singular();
    fn test_config_default_values();
    fn test_config_builder();
}
```

### types/bench.rs

```rust
#[cfg(feature = "bench")]
mod bench {
    fn bench_transform_point(c: &mut Criterion);
    fn bench_matrix_inverse(c: &mut Criterion);
    fn bench_matrix_compose(c: &mut Criterion);
    fn bench_transform_1000_points(c: &mut Criterion);
}
```

---

## Phase 2: Triangle Matching Algorithm

### 2.1 Algorithm Description

The triangle matching algorithm identifies similar star patterns between images:

1. **Triangle Formation**: Select N brightest stars, form triangles from all combinations
2. **Triangle Descriptor**: Compute invariant features (side ratios) independent of scale/rotation
3. **Hash Table Lookup**: Use geometric hashing for O(1) candidate retrieval
4. **Voting**: Accumulate votes for consistent star correspondences
5. **Verification**: Validate top candidates with geometric consistency

### 2.2 Implementation

```rust
/// Triangle formed from three stars with scale-invariant descriptors
#[derive(Debug, Clone)]
pub struct TriangleDescriptor {
    pub star_indices: [usize; 3],
    pub sides: [f64; 3],        // Sorted: a <= b <= c
    pub ratios: (f64, f64),     // (a/c, b/c) - scale invariant
    pub orientation: bool,       // Clockwise or counter-clockwise
}

impl TriangleDescriptor {
    pub fn from_positions(indices: [usize; 3], positions: [(f64, f64); 3]) -> Option<Self>;
    pub fn is_similar(&self, other: &Self, tolerance: f64) -> bool;
    pub fn hash_key(&self, bins: usize) -> (usize, usize);
}

/// Geometric hash table for O(1) triangle lookup
pub struct TriangleHashTable {
    table: Vec<Vec<usize>>,
    bins: usize,
}

impl TriangleHashTable {
    pub fn build(triangles: &[TriangleDescriptor], bins: usize) -> Self;
    pub fn find_candidates(&self, query: &TriangleDescriptor, tolerance: f64) -> Vec<usize>;
}

/// Configuration for triangle matching
#[derive(Debug, Clone)]
pub struct TriangleMatchConfig {
    pub max_stars: usize,           // Default: 50
    pub ratio_tolerance: f64,       // Default: 0.002
    pub min_votes: usize,           // Default: 3
    pub hash_bins: usize,           // Default: 100
    pub check_orientation: bool,    // Default: true
}

/// Match stars between reference and target using triangle matching
pub fn match_stars(
    ref_stars: &[(f64, f64)],
    target_stars: &[(f64, f64)],
    config: &TriangleMatchConfig,
) -> Vec<(usize, usize, usize)>;  // (ref_idx, target_idx, votes)
```

### 2.3 SIMD Optimizations

```rust
// triangle/simd/mod.rs
pub mod simd {
    /// SIMD-accelerated distance calculations for triangle formation
    /// Computes distances between all star pairs in parallel
    
    #[cfg(target_arch = "x86_64")]
    pub fn compute_distances_sse(
        positions: &[(f64, f64)],
        distances: &mut [f64],
    );
    
    #[cfg(target_arch = "aarch64")]
    pub fn compute_distances_neon(
        positions: &[(f64, f64)],
        distances: &mut [f64],
    );
    
    /// SIMD-accelerated ratio comparison for hash table lookup
    pub fn compare_ratios_simd(
        query: (f64, f64),
        candidates: &[(f64, f64)],
        tolerance: f64,
        matches: &mut [bool],
    );
}
```

### triangle/tests.rs

```rust
#[cfg(test)]
mod tests {
    // Triangle descriptor tests
    fn test_triangle_from_positions();
    fn test_triangle_ratios_scale_invariant();
    fn test_triangle_ratios_rotation_invariant();
    fn test_triangle_similarity_check();
    fn test_triangle_orientation_detection();
    fn test_degenerate_triangle_detection();
    fn test_collinear_points_rejected();
    fn test_very_small_triangle_rejected();
    
    // Hash table tests
    fn test_hash_table_build();
    fn test_hash_table_lookup_exact();
    fn test_hash_table_lookup_with_tolerance();
    fn test_hash_table_empty();
    fn test_hash_key_distribution();
    
    // Star matching tests
    fn test_match_identical_star_lists();
    fn test_match_translated_stars();
    fn test_match_rotated_stars_45deg();
    fn test_match_rotated_stars_90deg();
    fn test_match_rotated_stars_180deg();
    fn test_match_scaled_stars();
    fn test_match_similarity_transform();
    fn test_match_with_missing_stars();
    fn test_match_with_extra_stars();
    fn test_match_with_noise();
    fn test_match_mirrored_image();
    
    // Edge cases
    fn test_too_few_stars();
    fn test_all_collinear_stars();
    fn test_duplicate_positions();
    fn test_very_close_stars();
    
    // SIMD tests
    fn test_simd_distances_match_scalar();
    fn test_simd_ratio_compare_match_scalar();
}
```

### triangle/bench.rs

```rust
#[cfg(feature = "bench")]
mod bench {
    use criterion::{criterion_group, Criterion, BenchmarkId};
    
    // Triangle formation benchmarks
    fn bench_triangle_formation(c: &mut Criterion) {
        let mut group = c.benchmark_group("triangle_formation");
        for n_stars in [20, 50, 100, 200].iter() {
            group.bench_with_input(
                BenchmarkId::new("stars", n_stars),
                n_stars,
                |b, &n| { /* ... */ },
            );
        }
        group.finish();
    }
    
    // Hash table benchmarks
    fn bench_hash_table_build(c: &mut Criterion) {
        let mut group = c.benchmark_group("hash_table_build");
        for n_triangles in [1000, 5000, 20000].iter() {
            group.bench_with_input(
                BenchmarkId::new("triangles", n_triangles),
                n_triangles,
                |b, &n| { /* ... */ },
            );
        }
        group.finish();
    }
    
    fn bench_hash_table_lookup(c: &mut Criterion);
    
    // Full matching benchmarks
    fn bench_match_stars(c: &mut Criterion) {
        let mut group = c.benchmark_group("match_stars");
        for (n_ref, n_target) in [(50, 50), (100, 100), (200, 200)].iter() {
            // Translation only
            group.bench_function(
                BenchmarkId::new("translated", format!("{}x{}", n_ref, n_target)),
                |b| { /* ... */ },
            );
            // Rotation + scale
            group.bench_function(
                BenchmarkId::new("similarity", format!("{}x{}", n_ref, n_target)),
                |b| { /* ... */ },
            );
            // With 20% extra stars (outliers)
            group.bench_function(
                BenchmarkId::new("with_outliers", format!("{}x{}", n_ref, n_target)),
                |b| { /* ... */ },
            );
        }
        group.finish();
    }
    
    // SIMD comparison benchmarks
    fn bench_simd_vs_scalar_distances(c: &mut Criterion);
    
    criterion_group!(benches, 
        bench_triangle_formation,
        bench_hash_table_build,
        bench_hash_table_lookup,
        bench_match_stars,
        bench_simd_vs_scalar_distances,
    );
}
```

---

## Phase 3: RANSAC Transformation Estimation

### 3.1 Algorithm Description

RANSAC (Random Sample Consensus) robustly estimates transformations:

1. **Random Sample**: Select minimum points needed for transform
2. **Model Estimation**: Compute candidate transformation from sample
3. **Consensus**: Count inliers within threshold
4. **Iteration**: Repeat with adaptive termination
5. **Refinement**: Least-squares fit on all inliers

### 3.2 Implementation

```rust
#[derive(Debug, Clone)]
pub struct RansacConfig {
    pub max_iterations: usize,      // Default: 1000
    pub inlier_threshold: f64,      // Default: 2.0 pixels
    pub confidence: f64,            // Default: 0.999
    pub min_inlier_ratio: f64,      // Default: 0.5
    pub seed: Option<u64>,          // For reproducibility
}

#[derive(Debug, Clone)]
pub struct RansacResult {
    pub transform: TransformMatrix,
    pub inliers: Vec<usize>,
    pub inlier_ratio: f64,
    pub iterations: usize,
}

/// Estimate transformation using RANSAC
pub fn estimate_transform(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform_type: TransformType,
    config: &RansacConfig,
) -> Option<RansacResult>;

/// Refine transformation using weighted least squares on inliers
pub fn refine_transform(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    inliers: &[usize],
    transform_type: TransformType,
) -> TransformMatrix;

/// Minimum samples for each transform type
pub fn min_samples(transform_type: TransformType) -> usize {
    match transform_type {
        TransformType::Translation => 1,
        TransformType::Euclidean => 2,
        TransformType::Similarity => 2,
        TransformType::Affine => 3,
        TransformType::Homography => 4,
    }
}
```

### 3.3 SIMD Optimizations

```rust
// ransac/simd/mod.rs
pub mod simd {
    /// SIMD-accelerated residual computation
    /// Computes distance^2 for all points after transformation
    
    #[cfg(target_arch = "x86_64")]
    pub fn compute_residuals_sse(
        ref_points: &[(f64, f64)],
        target_points: &[(f64, f64)],
        transform: &TransformMatrix,
        residuals: &mut [f64],
    );
    
    #[cfg(target_arch = "aarch64")]
    pub fn compute_residuals_neon(
        ref_points: &[(f64, f64)],
        target_points: &[(f64, f64)],
        transform: &TransformMatrix,
        residuals: &mut [f64],
    );
    
    /// SIMD-accelerated inlier counting
    pub fn count_inliers_simd(
        residuals: &[f64],
        threshold_sq: f64,
    ) -> usize;
    
    /// SIMD-accelerated point centroid calculation
    pub fn compute_centroid_simd(
        points: &[(f64, f64)],
    ) -> (f64, f64);
}
```

### ransac/tests.rs

```rust
#[cfg(test)]
mod tests {
    // Basic estimation tests
    fn test_ransac_perfect_translation();
    fn test_ransac_perfect_euclidean();
    fn test_ransac_perfect_similarity();
    fn test_ransac_perfect_affine();
    fn test_ransac_perfect_homography();
    
    // Outlier rejection tests
    fn test_ransac_10_percent_outliers();
    fn test_ransac_20_percent_outliers();
    fn test_ransac_30_percent_outliers();
    fn test_ransac_50_percent_outliers();
    fn test_ransac_clustered_outliers();
    fn test_ransac_systematic_outliers();
    
    // Edge cases
    fn test_ransac_insufficient_points();
    fn test_ransac_all_outliers();
    fn test_ransac_collinear_points();
    fn test_ransac_degenerate_config();
    
    // Adaptive iteration tests
    fn test_adaptive_iterations_high_inlier_ratio();
    fn test_adaptive_iterations_low_inlier_ratio();
    fn test_early_termination();
    
    // Refinement tests
    fn test_refinement_improves_accuracy();
    fn test_refinement_affine_overdetermined();
    fn test_refinement_homography_overdetermined();
    
    // Reproducibility tests
    fn test_deterministic_with_seed();
    
    // SIMD tests
    fn test_simd_residuals_match_scalar();
    fn test_simd_inlier_count_match_scalar();
    fn test_simd_centroid_match_scalar();
}
```

### ransac/bench.rs

```rust
#[cfg(feature = "bench")]
mod bench {
    use criterion::{criterion_group, Criterion, BenchmarkId};
    
    fn bench_ransac_estimation(c: &mut Criterion) {
        let mut group = c.benchmark_group("ransac_estimation");
        
        // Varying point counts
        for n_points in [50, 100, 200, 500].iter() {
            // Different outlier ratios
            for outlier_pct in [0, 10, 20, 30].iter() {
                group.bench_function(
                    BenchmarkId::new(
                        format!("{}pct_outliers", outlier_pct),
                        n_points,
                    ),
                    |b| { /* ... */ },
                );
            }
        }
        group.finish();
    }
    
    fn bench_transform_types(c: &mut Criterion) {
        let mut group = c.benchmark_group("ransac_transform_types");
        for transform_type in [
            TransformType::Translation,
            TransformType::Similarity,
            TransformType::Affine,
            TransformType::Homography,
        ].iter() {
            group.bench_function(
                BenchmarkId::new("type", format!("{:?}", transform_type)),
                |b| { /* ... */ },
            );
        }
        group.finish();
    }
    
    fn bench_least_squares_refinement(c: &mut Criterion);
    fn bench_simd_vs_scalar_residuals(c: &mut Criterion);
    
    criterion_group!(benches,
        bench_ransac_estimation,
        bench_transform_types,
        bench_least_squares_refinement,
        bench_simd_vs_scalar_residuals,
    );
}
```

---

## Phase 4: Phase Correlation (Coarse Alignment)

### 4.1 Algorithm Description

Phase correlation uses FFT to find translation between images:

1. **FFT**: Compute 2D FFT of both images with Hann windowing
2. **Cross-Power Spectrum**: Multiply FFT1 by conjugate of FFT2, normalize
3. **Inverse FFT**: Peak location gives translation offset
4. **Sub-pixel Refinement**: Parabolic/Gaussian fit for sub-pixel accuracy

### 4.2 Implementation

```rust
#[derive(Debug, Clone)]
pub struct PhaseCorrelationConfig {
    pub use_windowing: bool,        // Default: true
    pub subpixel_method: SubpixelMethod,
    pub min_peak_value: f64,        // Default: 0.1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubpixelMethod {
    None,
    Parabolic,   // Fast, ~0.1 pixel accuracy
    Gaussian,    // Slower, ~0.05 pixel accuracy
    Centroid,    // Robust to noise
}

#[derive(Debug, Clone)]
pub struct PhaseCorrelationResult {
    pub translation: (f64, f64),
    pub peak_value: f64,
    pub confidence: f64,
}

/// Estimate translation using phase correlation
pub fn correlate(
    reference: &[f32],
    target: &[f32],
    width: usize,
    height: usize,
    config: &PhaseCorrelationConfig,
) -> PhaseCorrelationResult;

/// Apply Hann window to reduce edge artifacts
pub fn apply_hann_window(image: &mut [f32], width: usize, height: usize);

/// Find peak and refine to sub-pixel
pub fn find_peak_subpixel(
    correlation: &[f32],
    width: usize,
    height: usize,
    method: SubpixelMethod,
) -> (f64, f64, f64);
```

### 4.3 SIMD Optimizations

```rust
// phase_correlation/simd/mod.rs
pub mod simd {
    /// SIMD-accelerated complex multiply-conjugate for cross-power spectrum
    /// Computes (a + bi) * (c - di) = (ac + bd) + (bc - ad)i
    
    #[cfg(target_arch = "x86_64")]
    pub fn complex_multiply_conjugate_sse(
        fft1: &[Complex<f32>],
        fft2: &[Complex<f32>],
        output: &mut [Complex<f32>],
    );
    
    #[cfg(target_arch = "aarch64")]
    pub fn complex_multiply_conjugate_neon(
        fft1: &[Complex<f32>],
        fft2: &[Complex<f32>],
        output: &mut [Complex<f32>],
    );
    
    /// SIMD-accelerated normalization of cross-power spectrum
    pub fn normalize_spectrum_simd(
        spectrum: &mut [Complex<f32>],
    );
    
    /// SIMD-accelerated Hann window application
    pub fn apply_hann_window_simd(
        image: &mut [f32],
        width: usize,
        height: usize,
    );
    
    /// SIMD-accelerated peak search
    pub fn find_max_simd(
        data: &[f32],
    ) -> (usize, f32);
}
```

### phase_correlation/tests.rs

```rust
#[cfg(test)]
mod tests {
    // Basic correlation tests
    fn test_correlate_identical_images();
    fn test_correlate_translated_1_pixel();
    fn test_correlate_translated_5_pixels();
    fn test_correlate_translated_10_pixels();
    fn test_correlate_translated_subpixel();
    fn test_correlate_negative_translation();
    fn test_correlate_xy_translation();
    
    // Sub-pixel method tests
    fn test_subpixel_parabolic_accuracy();
    fn test_subpixel_gaussian_accuracy();
    fn test_subpixel_centroid_accuracy();
    fn test_subpixel_methods_comparison();
    
    // Windowing tests
    fn test_hann_window_application();
    fn test_windowing_reduces_artifacts();
    
    // Robustness tests
    fn test_correlate_with_noise();
    fn test_correlate_partial_overlap();
    fn test_correlate_different_brightness();
    fn test_correlate_empty_image();
    fn test_correlate_size_mismatch();
    
    // Confidence tests
    fn test_confidence_high_correlation();
    fn test_confidence_low_correlation();
    fn test_confidence_ambiguous_peaks();
    
    // SIMD tests
    fn test_simd_complex_multiply_matches_scalar();
    fn test_simd_normalize_matches_scalar();
    fn test_simd_hann_window_matches_scalar();
    fn test_simd_find_max_matches_scalar();
}
```

### phase_correlation/bench.rs

```rust
#[cfg(feature = "bench")]
mod bench {
    use criterion::{criterion_group, Criterion, BenchmarkId, Throughput};
    
    fn bench_phase_correlation_by_size(c: &mut Criterion) {
        let mut group = c.benchmark_group("phase_correlation");
        
        for (width, height) in [
            (256, 256),
            (512, 512),
            (1024, 1024),
            (2048, 2048),
            (4096, 4096),
        ].iter() {
            let pixels = width * height;
            group.throughput(Throughput::Elements(*pixels as u64));
            group.bench_with_input(
                BenchmarkId::new("size", format!("{}x{}", width, height)),
                &(*width, *height),
                |b, &(w, h)| { /* ... */ },
            );
        }
        group.finish();
    }
    
    fn bench_fft_operations(c: &mut Criterion) {
        let mut group = c.benchmark_group("fft_operations");
        for size in [512, 1024, 2048, 4096].iter() {
            group.bench_function(
                BenchmarkId::new("forward_2d", size),
                |b| { /* ... */ },
            );
            group.bench_function(
                BenchmarkId::new("inverse_2d", size),
                |b| { /* ... */ },
            );
        }
        group.finish();
    }
    
    fn bench_subpixel_methods(c: &mut Criterion);
    fn bench_hann_window(c: &mut Criterion);
    fn bench_simd_vs_scalar_complex_multiply(c: &mut Criterion);
    
    criterion_group!(benches,
        bench_phase_correlation_by_size,
        bench_fft_operations,
        bench_subpixel_methods,
        bench_hann_window,
        bench_simd_vs_scalar_complex_multiply,
    );
}
```

---

## Phase 5: Image Interpolation

### 5.1 Implementation

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpolationMethod {
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos2,  // a=2, 4x4 kernel
    Lanczos3,  // a=3, 6x6 kernel (default, best quality)
}

impl InterpolationMethod {
    pub fn kernel_radius(&self) -> usize;
}

/// Interpolate at sub-pixel position
pub fn interpolate(
    image: &[f32],
    width: usize,
    height: usize,
    x: f64,
    y: f64,
    method: InterpolationMethod,
) -> f32;

/// Lanczos kernel computation
pub fn lanczos_kernel(x: f64, a: usize) -> f64 {
    if x == 0.0 {
        1.0
    } else if x.abs() < a as f64 {
        let pi_x = std::f64::consts::PI * x;
        let pi_x_a = pi_x / a as f64;
        (pi_x.sin() / pi_x) * (pi_x_a.sin() / pi_x_a)
    } else {
        0.0
    }
}

/// Bicubic kernel (Catmull-Rom)
pub fn bicubic_kernel(x: f64) -> f64;

/// Warp image using transformation
pub fn warp_image(
    src: &[f32],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
    transform: &TransformMatrix,
    method: InterpolationMethod,
) -> Vec<f32>;

/// Resample image to new dimensions
pub fn resample(
    src: &[f32],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
    method: InterpolationMethod,
) -> Vec<f32>;
```

### 5.2 SIMD Optimizations

```rust
// interpolation/simd/mod.rs
pub mod simd {
    /// SIMD-accelerated Lanczos interpolation for a row of output pixels
    /// Processes 4/8 output pixels in parallel
    
    #[cfg(target_arch = "x86_64")]
    pub fn interpolate_row_lanczos_sse(
        src: &[f32],
        src_width: usize,
        src_height: usize,
        dst: &mut [f32],
        y: usize,
        x_coords: &[f64],
        y_coord: f64,
    );
    
    #[cfg(target_arch = "aarch64")]
    pub fn interpolate_row_lanczos_neon(
        src: &[f32],
        src_width: usize,
        src_height: usize,
        dst: &mut [f32],
        y: usize,
        x_coords: &[f64],
        y_coord: f64,
    );
    
    /// SIMD-accelerated bilinear interpolation
    /// Much faster than Lanczos, good for previews
    pub fn interpolate_bilinear_simd(
        src: &[f32],
        src_width: usize,
        x: &[f32; 8],
        y: &[f32; 8],
    ) -> [f32; 8];
    
    /// SIMD-accelerated kernel weight computation
    pub fn compute_lanczos_weights_simd(
        offsets: &[f64],
        a: usize,
        weights: &mut [f64],
    );
}
```

### interpolation/tests.rs

```rust
#[cfg(test)]
mod tests {
    // Kernel property tests
    fn test_lanczos_kernel_center_is_one();
    fn test_lanczos_kernel_zeros_at_integers();
    fn test_lanczos_kernel_symmetry();
    fn test_lanczos_kernel_outside_window();
    fn test_lanczos2_vs_lanczos3();
    fn test_bicubic_kernel_properties();
    
    // Interpolation accuracy tests
    fn test_nearest_at_pixel_centers();
    fn test_bilinear_gradient();
    fn test_bilinear_at_pixel_centers();
    fn test_bicubic_smooth_curve();
    fn test_bicubic_preserves_dc();
    fn test_lanczos_preserves_dc();
    fn test_lanczos_smooth_gradient();
    
    // Edge handling tests
    fn test_interpolation_near_edge();
    fn test_interpolation_at_boundary();
    fn test_border_handling_modes();
    
    // Warp tests
    fn test_warp_identity();
    fn test_warp_translation_integer();
    fn test_warp_translation_subpixel();
    fn test_warp_rotation_90();
    fn test_warp_rotation_45();
    fn test_warp_scale_2x();
    fn test_warp_scale_half();
    fn test_warp_affine();
    fn test_warp_homography();
    
    // Resample tests
    fn test_resample_upscale_2x();
    fn test_resample_downscale_half();
    fn test_resample_arbitrary();
    
    // SIMD tests
    fn test_simd_lanczos_matches_scalar();
    fn test_simd_bilinear_matches_scalar();
    fn test_simd_weights_match_scalar();
}
```

### interpolation/bench.rs

```rust
#[cfg(feature = "bench")]
mod bench {
    use criterion::{criterion_group, Criterion, BenchmarkId, Throughput};
    
    fn bench_interpolation_methods(c: &mut Criterion) {
        let mut group = c.benchmark_group("interpolation_methods");
        
        let size = 1024;
        let pixels = size * size;
        group.throughput(Throughput::Elements(pixels as u64));
        
        for method in [
            InterpolationMethod::Nearest,
            InterpolationMethod::Bilinear,
            InterpolationMethod::Bicubic,
            InterpolationMethod::Lanczos2,
            InterpolationMethod::Lanczos3,
        ].iter() {
            group.bench_function(
                BenchmarkId::new("method", format!("{:?}", method)),
                |b| { /* ... */ },
            );
        }
        group.finish();
    }
    
    fn bench_warp_by_size(c: &mut Criterion) {
        let mut group = c.benchmark_group("warp_image");
        
        for size in [512, 1024, 2048, 4096].iter() {
            let pixels = size * size;
            group.throughput(Throughput::Elements(pixels as u64));
            
            group.bench_function(
                BenchmarkId::new("translation", size),
                |b| { /* ... */ },
            );
            group.bench_function(
                BenchmarkId::new("rotation", size),
                |b| { /* ... */ },
            );
            group.bench_function(
                BenchmarkId::new("affine", size),
                |b| { /* ... */ },
            );
        }
        group.finish();
    }
    
    fn bench_resample(c: &mut Criterion);
    fn bench_simd_vs_scalar(c: &mut Criterion);
    fn bench_parallel_vs_sequential(c: &mut Criterion);
    
    criterion_group!(benches,
        bench_interpolation_methods,
        bench_warp_by_size,
        bench_resample,
        bench_simd_vs_scalar,
        bench_parallel_vs_sequential,
    );
}
```

---

## Phase 6: Full Pipeline Integration

### 6.1 Implementation

```rust
/// High-level registrator with caching and configuration
pub struct Registrator {
    config: RegistrationConfig,
}

impl Registrator {
    pub fn new(config: RegistrationConfig) -> Self;
    pub fn with_defaults() -> Self;
    
    /// Register stars only (no image warping)
    pub fn register_stars(
        &self,
        ref_stars: &[(f64, f64)],
        target_stars: &[(f64, f64)],
    ) -> Result<RegistrationResult, RegistrationError>;
    
    /// Quick registration using phase correlation only
    pub fn quick_register(
        &self,
        reference: &[f32],
        target: &[f32],
        width: usize,
        height: usize,
    ) -> Result<RegistrationResult, RegistrationError>;
    
    /// Warp target image to match reference
    pub fn warp_to_reference(
        &self,
        target: &[f32],
        width: usize,
        height: usize,
        transform: &TransformMatrix,
        method: InterpolationMethod,
    ) -> Vec<f32>;
}

/// Builder pattern for configuration
pub struct RegistrationConfigBuilder { /* ... */ }

impl RegistrationConfigBuilder {
    pub fn new() -> Self;
    pub fn translation_only(self) -> Self;
    pub fn with_rotation(self) -> Self;
    pub fn with_scale(self) -> Self;
    pub fn full_affine(self) -> Self;
    pub fn full_homography(self) -> Self;
    pub fn use_phase_correlation(self, enable: bool) -> Self;
    pub fn ransac_iterations(self, n: usize) -> Self;
    pub fn ransac_threshold(self, pixels: f64) -> Self;
    pub fn max_stars(self, n: usize) -> Self;
    pub fn min_matched_stars(self, n: usize) -> Self;
    pub fn build(self) -> RegistrationConfig;
}

#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("Insufficient stars: found {found}, need {required}")]
    InsufficientStars { found: usize, required: usize },
    
    #[error("No matching star patterns found")]
    NoMatchingPatterns,
    
    #[error("RANSAC failed to find valid transformation")]
    RansacFailed { reason: String },
    
    #[error("Registration accuracy too low: {rms_error:.3} > {max_allowed:.3} pixels")]
    AccuracyTooLow { rms_error: f64, max_allowed: f64 },
    
    #[error("Image dimension mismatch")]
    DimensionMismatch,
}
```

### pipeline/tests.rs

```rust
#[cfg(test)]
mod tests {
    // End-to-end registration tests
    fn test_register_identity();
    fn test_register_translation_only();
    fn test_register_small_rotation();
    fn test_register_large_rotation();
    fn test_register_similarity();
    fn test_register_affine();
    fn test_register_with_outliers();
    fn test_register_quality_metrics();
    
    // Quick registration tests
    fn test_quick_register_translation();
    fn test_quick_register_large_shift();
    
    // Warp tests
    fn test_warp_to_reference();
    fn test_warp_preserves_content();
    
    // Builder tests
    fn test_builder_default();
    fn test_builder_presets();
    fn test_builder_custom_config();
    
    // Error handling tests
    fn test_error_insufficient_stars();
    fn test_error_no_matching_patterns();
    fn test_error_ransac_failed();
    fn test_error_dimension_mismatch();
    
    // Integration tests (with star detection)
    #[ignore]
    fn test_full_pipeline_synthetic_stars();
    #[ignore]
    fn test_full_pipeline_real_image();
}
```

### pipeline/bench.rs

```rust
#[cfg(feature = "bench")]
mod bench {
    use criterion::{criterion_group, Criterion, BenchmarkId, Throughput};
    
    fn bench_full_pipeline(c: &mut Criterion) {
        let mut group = c.benchmark_group("full_pipeline");
        
        for (size, n_stars) in [
            (1024, 50),
            (2048, 100),
            (4096, 200),
        ].iter() {
            let pixels = size * size;
            group.throughput(Throughput::Elements(pixels as u64));
            
            group.bench_function(
                BenchmarkId::new("register", format!("{}x{}_{}stars", size, size, n_stars)),
                |b| { /* ... */ },
            );
        }
        group.finish();
    }
    
    fn bench_pipeline_stages(c: &mut Criterion) {
        let mut group = c.benchmark_group("pipeline_stages");
        
        // Measure each stage separately
        group.bench_function("triangle_matching", |b| { /* ... */ });
        group.bench_function("ransac_estimation", |b| { /* ... */ });
        group.bench_function("phase_correlation", |b| { /* ... */ });
        group.bench_function("image_warping", |b| { /* ... */ });
        
        group.finish();
    }
    
    fn bench_config_variations(c: &mut Criterion);
    
    criterion_group!(benches,
        bench_full_pipeline,
        bench_pipeline_stages,
        bench_config_variations,
    );
}
```

---

## Phase 7: Quality Metrics and Validation

### 7.1 Implementation

```rust
#[derive(Debug, Clone)]
pub struct QualityMetrics {
    pub rms_error: f64,
    pub max_error: f64,
    pub mean_error: f64,
    pub median_error: f64,
    pub percentile_95: f64,
    pub inlier_ratio: f64,
    pub match_ratio: f64,
    pub is_acceptable: bool,
}

#[derive(Debug, Clone)]
pub struct ResidualStats {
    pub mean: f64,
    pub std_dev: f64,
    pub min: f64,
    pub max: f64,
    pub median: f64,
}

#[derive(Debug, Clone)]
pub struct QuadrantConsistency {
    pub quadrant_rms: [f64; 4],
    pub max_deviation: f64,
    pub is_uniform: bool,
}

/// Compute comprehensive quality metrics
pub fn compute_quality_metrics(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    inliers: &[usize],
) -> QualityMetrics;

/// Compute residual statistics
pub fn compute_residual_stats(residuals: &[f64]) -> ResidualStats;

/// Check consistency across image quadrants
pub fn check_quadrant_consistency(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    width: usize,
    height: usize,
) -> QuadrantConsistency;

/// Estimate overlap between images
pub fn estimate_overlap(
    transform: &TransformMatrix,
    width: usize,
    height: usize,
) -> f64;
```

### quality/tests.rs

```rust
#[cfg(test)]
mod tests {
    // Quality metrics tests
    fn test_quality_metrics_perfect();
    fn test_quality_metrics_high_error();
    fn test_quality_metrics_low_inlier_ratio();
    fn test_quality_metrics_threshold();
    
    // Residual stats tests
    fn test_residual_stats_basic();
    fn test_residual_stats_empty();
    fn test_residual_stats_single();
    fn test_percentile_computation();
    
    // Quadrant consistency tests
    fn test_quadrant_consistency_uniform();
    fn test_quadrant_consistency_non_uniform();
    fn test_quadrant_consistency_missing_quadrant();
    
    // Overlap estimation tests
    fn test_overlap_identity();
    fn test_overlap_translation();
    fn test_overlap_large_shift();
    fn test_overlap_rotation();
}
```

### quality/bench.rs

```rust
#[cfg(feature = "bench")]
mod bench {
    use criterion::{criterion_group, Criterion, BenchmarkId};
    
    fn bench_quality_metrics(c: &mut Criterion) {
        let mut group = c.benchmark_group("quality_metrics");
        
        for n_points in [50, 100, 500, 1000].iter() {
            group.bench_with_input(
                BenchmarkId::new("compute_metrics", n_points),
                n_points,
                |b, &n| { /* ... */ },
            );
        }
        group.finish();
    }
    
    fn bench_residual_computation(c: &mut Criterion);
    fn bench_quadrant_consistency(c: &mut Criterion);
    
    criterion_group!(benches,
        bench_quality_metrics,
        bench_residual_computation,
        bench_quadrant_consistency,
    );
}
```

---

## SIMD Implementation Summary

### SSE4.1 (x86_64 baseline)

| Module | Function | Description |
|--------|----------|-------------|
| triangle | `compute_distances_sse` | Pairwise distance calculation |
| triangle | `compare_ratios_sse` | Ratio tolerance checking |
| ransac | `compute_residuals_sse` | Transform residual calculation |
| ransac | `count_inliers_sse` | Threshold-based counting |
| phase_correlation | `complex_multiply_conjugate_sse` | Cross-power spectrum |
| phase_correlation | `normalize_spectrum_sse` | Magnitude normalization |
| phase_correlation | `apply_hann_window_sse` | Window function |
| interpolation | `interpolate_row_lanczos_sse` | Lanczos row processing |
| interpolation | `interpolate_bilinear_sse` | Fast bilinear |

### NEON (ARM64)

All above functions have NEON equivalents with `_neon` suffix.

### Runtime Detection

```rust
// In each simd/mod.rs
pub fn compute_distances(positions: &[(f64, f64)], distances: &mut [f64]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse4.1") {
            return unsafe { sse::compute_distances_sse(positions, distances) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { neon::compute_distances_neon(positions, distances) };
    }
    // Scalar fallback
    compute_distances_scalar(positions, distances)
}
```

---

## Benchmark Organization

### Benchmark Files Location

```
lumos/benches/
|-- registration_triangle.rs      # Triangle matching benchmarks
|-- registration_ransac.rs        # RANSAC benchmarks
|-- registration_phase_corr.rs    # Phase correlation benchmarks
|-- registration_interpolation.rs # Interpolation benchmarks
|-- registration_pipeline.rs      # Full pipeline benchmarks
+-- registration_quality.rs       # Quality metrics benchmarks
```

### Cargo.toml Bench Configuration

```toml
[[bench]]
name = "registration_triangle"
harness = false
required-features = ["bench"]

[[bench]]
name = "registration_ransac"
harness = false
required-features = ["bench"]

[[bench]]
name = "registration_phase_corr"
harness = false
required-features = ["bench"]

[[bench]]
name = "registration_interpolation"
harness = false
required-features = ["bench"]

[[bench]]
name = "registration_pipeline"
harness = false
required-features = ["bench"]

[[bench]]
name = "registration_quality"
harness = false
required-features = ["bench"]
```

---

## Dependencies

```toml
[dependencies]
# FFT for phase correlation
rustfft = { workspace = true }

# Parallel processing
rayon = { workspace = true }

# Error handling
thiserror = { workspace = true }

# Random number generation for RANSAC
rand = { workspace = true }
rand_chacha = { workspace = true }

[dev-dependencies]
# Benchmarking
criterion = { workspace = true }
```

---

## Success Criteria

| Metric | Target | Stretch Goal |
|--------|--------|--------------|
| Sub-pixel accuracy | < 0.1 pixel RMS | < 0.05 pixel RMS |
| Matching success rate | > 95% | > 99% |
| Processing time (2K x 2K) | < 500ms | < 100ms |
| Processing time (4K x 4K) | < 2s | < 500ms |
| Outlier tolerance | 30% | 50% |
| Rotation detection | +/-180 deg | Any angle |
| Scale detection | 0.5x - 2.0x | 0.1x - 10x |
| Memory usage | < 4x image size | < 2x image size |
| SIMD speedup | 2-4x | 4-8x |

---

## References

### Academic Papers
1. Beroiz, M., Cabral, J. B., & Sanchez, B. (2020). Astroalign: A Python module for 
   astronomical image registration. Astronomy and Computing, 32, 100384.
2. Fischler, M. A., & Bolles, R. C. (1981). Random sample consensus: a paradigm for 
   model fitting. Communications of the ACM, 24(6), 381-395.
3. Kuglin, C. D., & Hines, D. C. (1975). The phase correlation image alignment method.
   IEEE Conference on Cybernetics and Society.

### Software References
- Siril Registration: https://siril.readthedocs.io/en/latest/preprocessing/registration.html
- PixInsight StarAlignment: https://www.pixinsight.com/tutorials/sa-distortion/
- Astroalign Python: https://astroalign.quatrope.org/

### Rust Crates
- rustfft: https://crates.io/crates/rustfft
- rayon: https://crates.io/crates/rayon
