# Image Registration Module

This module provides production-grade image alignment for astrophotography, supporting the complete workflow from calibrated light frames through final stacked images.

## Table of Contents

1. [Overview](#overview)
2. [Using Calibrated Lights with Registration](#using-calibrated-lights-with-registration)
3. [Architecture](#architecture)
4. [Core Algorithms](#core-algorithms)
5. [Transform Types](#transform-types)
6. [Configuration](#configuration)
7. [Quality Metrics](#quality-metrics)
8. [Performance](#performance)

---

## Overview

The registration module aligns astronomical images by:

1. **Finding star correspondences** between reference and target images
2. **Estimating geometric transformations** that map target stars to reference positions
3. **Warping images** with high-quality interpolation to produce aligned output

```
Input Images → Star Detection → Triangle Matching → RANSAC → Transform → Warp → Aligned Output
```

### Key Features

- **Robust star matching** using geometric hashing of triangle patterns
- **Outlier-tolerant estimation** via RANSAC with local optimization
- **Sub-pixel accuracy** through iterative refinement
- **Multiple transform types** from simple translation to full homography
- **High-quality interpolation** with Lanczos kernels
- **SIMD-optimized** critical paths (AVX2/SSE on x86_64, NEON on aarch64)

---

## Using Calibrated Lights with Registration

### Complete Astrophotography Workflow

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│ Light Frame │───►│ Calibration │───►│   Debayer   │
│   (RAW)     │    │ (bias/dark/ │    │  (if CFA)   │
└─────────────┘    │    flat)    │    └──────┬──────┘
                   └─────────────┘           │
                                             ▼
                   ┌─────────────┐    ┌─────────────┐
                   │   Aligned   │◄───│Registration │
                   │   Lights    │    │             │
                   └──────┬──────┘    └──────┬──────┘
                          │                  │
                          ▼                  │
                   ┌─────────────┐           │
                   │   Stacking  │           │
                   └─────────────┘           │
                                             │
                                    ┌────────┴────────┐
                                    │ Star Detection  │
                                    └─────────────────┘
```

### Step-by-Step Guide

#### 1. Calibrate Your Light Frames

Before registration, apply standard calibration:

```rust
// Pseudocode for calibration workflow
let calibrated = (light - master_dark - master_bias) / master_flat;
```

Registration works best on calibrated images because:
- Hot pixels are removed (they would be detected as false stars)
- Flat-field correction ensures uniform star brightness across the field
- Bias/dark subtraction improves SNR for faint star detection

#### 2. Detect Stars in Each Frame

```rust
use lumos::star_detection::{StarDetector, DetectionConfig};

let detector = StarDetector::new(DetectionConfig {
    detection_threshold: 5.0,    // 5-sigma above background
    min_separation: 5.0,         // pixels between stars
    max_stars: 500,              // limit for performance
    ..Default::default()
});

let reference_stars = detector.detect(&reference_image, width, height)?;
let target_stars = detector.detect(&target_image, width, height)?;
```

**Tips for star detection:**
- Use a threshold of 3-5 sigma for good detection without noise
- Set `min_separation` based on your seeing/FWHM
- More stars generally improves registration, but diminishing returns after ~200
- Stars are automatically sorted by brightness; registration uses the brightest

#### 3. Register Images

```rust
use lumos::registration::{
    Registrator, RegistrationConfig, TransformType,
    warp_to_reference, InterpolationMethod
};

// Configure registration
let config = RegistrationConfig::builder()
    .with_scale()                  // Similarity transform (translation + rotation + scale)
    .ransac_iterations(1000)
    .ransac_threshold(2.0)         // 2 pixel inlier threshold
    .max_stars(200)                // Use brightest 200 stars
    .build();

let registrator = Registrator::new(config);

// Extract star positions
let ref_positions: Vec<(f64, f64)> = reference_stars
    .iter()
    .map(|s| (s.x, s.y))
    .collect();

let target_positions: Vec<(f64, f64)> = target_stars
    .iter()
    .map(|s| (s.x, s.y))
    .collect();

// Find transformation
let result = registrator.register_star_positions(&ref_positions, &target_positions)?;

println!("Matched {} stars, RMS error: {:.3} pixels", 
    result.num_inliers, result.rms_error);
```

#### 4. Apply Transformation (Warp Image)

```rust
// Warp target image to align with reference
let aligned = warp_to_reference_image(
    &target_image,
    &result.transform,
    InterpolationMethod::Lanczos3,  // High-quality interpolation
);
```

#### 5. Stack Aligned Images

After all images are aligned to the reference, stack them:

```rust
// Simple mean stacking (pseudocode)
let stacked = aligned_images
    .iter()
    .fold(vec![0.0; size], |acc, img| {
        acc.iter().zip(img).map(|(a, b)| a + b).collect()
    })
    .iter()
    .map(|v| v / num_images as f32)
    .collect();
```

### Handling Different Scenarios

#### Large Offsets (Dithered Images)

For images with large offsets, enable phase correlation for coarse alignment:

```rust
let config = RegistrationConfig::builder()
    .with_scale()
    .use_phase_correlation(true)  // FFT-based coarse alignment first
    .ransac_iterations(1000)
    .build();
```

#### Field Rotation (Alt-Az Mounts)

For equatorial tracking errors or alt-az field rotation:

```rust
let config = RegistrationConfig::builder()
    .with_scale()  // Includes rotation
    .ransac_threshold(1.5)  // Tighter threshold
    .build();
```

#### Wide-Field Distortion

For images with optical distortion (especially at field edges):

```rust
use lumos::registration::distortion::{ThinPlateSpline, TpsConfig};

// First, get matched star pairs from registration
let result = registrator.register_star_positions(&ref_positions, &target_positions)?;

// Extract inlier pairs
let (ref_inliers, target_inliers): (Vec<_>, Vec<_>) = result
    .matched_stars
    .iter()
    .map(|&(ri, ti)| (ref_positions[ri], target_positions[ti]))
    .unzip();

// Fit thin-plate spline for local distortion correction
let tps = ThinPlateSpline::fit(
    &target_inliers,
    &ref_inliers,
    TpsConfig { regularization: 0.1 }  // Smooth interpolation
)?;

// Apply TPS warp for distortion correction
// (custom warping with tps.transform(x, y) for each pixel)
```

---

## Architecture

### Module Structure

```
registration/
├── mod.rs              # Public API, re-exports
├── pipeline/           # High-level registration orchestration
│   └── mod.rs          #   Registrator, MultiScaleRegistrator
├── triangle/           # Star matching via triangle hashing
│   └── mod.rs          #   TriangleMatcher, vote matrices
├── ransac/             # Robust transform estimation
│   ├── mod.rs          #   RansacEstimator, LO-RANSAC
│   └── simd/           #   SIMD-accelerated inlier counting
├── phase_correlation/  # FFT-based coarse alignment
│   └── mod.rs          #   PhaseCorrelator, log-polar transform
├── interpolation/      # Image warping
│   ├── mod.rs          #   Lanczos, bicubic, bilinear kernels
│   └── simd/           #   SIMD-accelerated warping
├── spatial/            # K-D tree for neighbor queries
├── distortion/         # Thin-plate splines
├── types/              # TransformMatrix, configs, results
├── quality/            # Quality metrics and validation
└── constants.rs        # Algorithm constants
```

### Data Flow

```
register_star_positions(ref_stars, target_stars)
    │
    ├─► Validate inputs (min star count, etc.)
    │
    ├─► Limit to brightest N stars (default: 200)
    │
    ├─► [Optional] Phase correlation for coarse alignment
    │
    ├─► Triangle matching
    │   ├── Form triangles from k-nearest neighbors
    │   ├── Hash by scale-invariant ratios
    │   ├── Match target triangles to reference
    │   └── Vote for star correspondences
    │
    ├─► RANSAC estimation
    │   ├── Sample minimal point sets
    │   ├── Estimate transformation
    │   ├── Count inliers (SIMD-accelerated)
    │   ├── [Optional] Local optimization
    │   └── Refine with all inliers
    │
    ├─► Quality validation
    │   ├── Check RMS error
    │   ├── Check inlier count
    │   └── Compute quality score
    │
    └─► Return RegistrationResult
```

---

## Core Algorithms

### Triangle Matching

**Purpose:** Find star correspondences without requiring initial alignment.

**How it works:**

1. **Form triangles** from each star and its k-nearest neighbors
2. **Compute descriptors**: For each triangle with sides s₀ ≤ s₁ ≤ s₂, compute ratios (s₀/s₂, s₁/s₂)
3. **Hash reference triangles** into 2D bins by their ratios
4. **Match target triangles**: Look up candidates in hash table, verify geometry
5. **Vote for correspondences**: Each matching triangle votes for 3 vertex pairs
6. **Resolve conflicts**: Greedy assignment ensures one-to-one matching

**Why triangles?**
- Scale-invariant (ratios don't change with image scale)
- Rotation-invariant (ratios don't change with rotation)
- Efficient O(n·k²) instead of O(n³) for all pairs

**Configuration:**
```rust
TriangleMatchConfig {
    max_stars: 200,           // Use brightest N
    ratio_tolerance: 0.01,    // 1% ratio difference
    min_votes: 3,             // Minimum votes for match
    hash_bins: 100,           // 100x100 hash table
    check_orientation: true,  // Reject mirrored matches
    two_step_matching: false, // Enable for difficult cases
}
```

### RANSAC (Random Sample Consensus)

**Purpose:** Robustly estimate transformation despite outliers.

**Algorithm:**

```
For iteration in 1..max_iterations:
    1. Sample minimum points for transform type
    2. Estimate transformation from sample
    3. Count inliers (points within threshold)
    4. If best so far, save model
    5. Update adaptive iteration count
    
Final: Refine with least squares on all inliers
```

**Local Optimization (LO-RANSAC):**

For promising models, iteratively re-estimate using current inliers:
- Typically improves inlier count by 5-15%
- Enabled by default

**Progressive Sampling:**

Samples preferentially from high-confidence matches early, then expands:
- Phase 1 (0-33%): Top 25% confidence matches
- Phase 2 (33-66%): Top 50%
- Phase 3 (66-100%): All matches

### Phase Correlation

**Purpose:** FFT-based coarse alignment for large offsets.

**Standard algorithm:**
1. Compute 2D FFT of both images
2. Cross-power spectrum: `(F₁ · conj(F₂)) / |F₁ · conj(F₂)|`
3. Inverse FFT → correlation surface
4. Find peak → integer pixel shift
5. Sub-pixel refinement (parabolic fitting)

**Rotation detection:**
- Convert magnitude spectra to log-polar coordinates
- Rotation becomes vertical shift
- Scale becomes horizontal shift

**When to use:**
- Large translations (>10% of image size)
- Unknown initial alignment
- Complement to star matching

### Interpolation (Image Warping)

**Available methods:**

| Method | Kernel Size | Quality | Speed | Use Case |
|--------|-------------|---------|-------|----------|
| Nearest | 1×1 | Poor | Fastest | Preview |
| Bilinear | 2×2 | Fair | Fast | Draft |
| Bicubic | 4×4 | Good | Medium | General |
| Lanczos2 | 4×4 | Very Good | Medium | Balanced |
| Lanczos3 | 6×6 | Excellent | Slow | **Default** |
| Lanczos4 | 8×8 | Best | Slowest | Maximum quality |

**Lanczos kernel:**
```
L(x) = sinc(x) · sinc(x/a)  for |x| < a
     = 0                     otherwise
```

Where `a` is the kernel radius (2, 3, or 4).

**Optimizations:**
- Pre-computed LUT with 1024 samples per unit
- SIMD-accelerated bilinear warping (1.6x speedup)

---

## Transform Types

| Type | DOF | Matrix Form | Use Case |
|------|-----|-------------|----------|
| **Translation** | 2 | `[1 0 tx; 0 1 ty; 0 0 1]` | Simple shifts |
| **Euclidean** | 3 | `[cos -sin tx; sin cos ty; 0 0 1]` | Rigid body |
| **Similarity** | 4 | `[s·cos -s·sin tx; s·sin s·cos ty; 0 0 1]` | **Most common** |
| **Affine** | 6 | `[a b tx; c d ty; 0 0 1]` | Shear/differential scale |
| **Homography** | 8 | `[a b c; d e f; g h 1]` | Full perspective |

**Minimum points required:**
- Translation: 1
- Euclidean/Similarity: 2
- Affine: 3
- Homography: 4

**Recommendation:** Use `Similarity` (with_scale) for most astrophotography. It handles:
- Translation (dithering, guiding errors)
- Rotation (field rotation, polar alignment errors)
- Scale (focus drift, atmospheric refraction)

---

## Configuration

### Quick Start Configurations

**Well-aligned images (small dither):**
```rust
let config = RegistrationConfig::builder()
    .with_scale()
    .ransac_iterations(500)
    .build();
```

**Large offsets:**
```rust
let config = RegistrationConfig::builder()
    .with_scale()
    .use_phase_correlation(true)
    .ransac_iterations(1000)
    .build();
```

**High accuracy:**
```rust
let config = RegistrationConfig::builder()
    .full_affine()
    .ransac_threshold(1.0)
    .ransac_iterations(2000)
    .max_residual(0.5)
    .build();
```

### All Configuration Options

```rust
RegistrationConfig {
    // Transform type
    transform_type: TransformType::Similarity,
    
    // RANSAC parameters
    ransac_iterations: 1000,      // Max iterations
    ransac_threshold: 2.0,        // Inlier threshold (pixels)
    ransac_confidence: 0.999,     // Confidence level
    use_local_optimization: true, // LO-RANSAC
    
    // Star selection
    min_stars_for_matching: 4,    // Minimum required
    max_stars_for_matching: 200,  // Use brightest N
    
    // Triangle matching
    triangle_tolerance: 0.01,     // Ratio tolerance (1%)
    min_matched_stars: 4,         // After matching
    
    // Quality thresholds
    max_residual_pixels: 5.0,     // Max acceptable RMS
    
    // Optional features
    use_phase_correlation: false, // Coarse alignment
}
```

---

## Quality Metrics

### RegistrationResult Fields

```rust
RegistrationResult {
    transform: TransformMatrix,           // The estimated transform
    matched_stars: Vec<(usize, usize)>,   // (ref_idx, target_idx) pairs
    residuals: Vec<f64>,                  // Per-match errors (pixels)
    rms_error: f64,                       // Root mean square error
    num_inliers: usize,                   // Stars used in final estimate
    quality_score: f64,                   // 0.0-1.0 composite score
    elapsed_ms: f64,                      // Processing time
}
```

### Interpreting Quality

| RMS Error | Quality | Notes |
|-----------|---------|-------|
| < 0.5 px | Excellent | Sub-pixel alignment |
| 0.5-1.0 px | Very Good | High-quality stacking |
| 1.0-2.0 px | Good | Standard quality |
| 2.0-5.0 px | Fair | May show artifacts |
| > 5.0 px | Poor | Check inputs |

### Quality Score Components

```
quality_score = 0.40 × error_score      // exp(-rms/2)
              + 0.25 × match_score      // min(inliers/50, 1)
              + 0.20 × inlier_ratio     // inliers/matches
              + 0.15 × overlap_score    // estimated overlap
```

---

## Performance

### Typical Processing Times

| Operation | 100 stars | 500 stars | Notes |
|-----------|-----------|-----------|-------|
| Triangle matching | ~1 ms | ~10 ms | O(n·k²) |
| RANSAC (1000 iter) | ~5 ms | ~20 ms | O(iter·n) |
| Phase correlation | ~2 ms | ~2 ms | O(N² log N), size-dependent |
| Lanczos3 warp | ~50 ms | ~50 ms | O(W×H), 1024×1024 image |

### SIMD Speedups

| Operation | Speedup | Notes |
|-----------|---------|-------|
| Inlier counting | 1.6x | AVX2/NEON |
| Bilinear warp | 1.6x | Row processing |

### Memory Usage

- Triangle hash table: ~10 KB for 100 stars
- Vote matrix: Dense up to 250K entries (~500 KB), sparse above
- Warp buffer: Output image size

---

## Troubleshooting

### Common Issues

**"InsufficientStars" error:**
- Lower star detection threshold
- Check image quality (focus, clouds)
- Ensure calibration didn't introduce artifacts

**"NoMatchingPatterns" error:**
- Images may not overlap
- Try `use_phase_correlation(true)` for large offsets
- Increase `triangle_tolerance`

**High RMS error:**
- Check for field distortion (use Affine or TPS)
- Verify stars are well-detected (not cosmic rays)
- Lower `ransac_threshold` for stricter inliers

**Slow performance:**
- Reduce `max_stars_for_matching`
- Use `Bilinear` interpolation for preview
- Enable multi-scale for large images

### Debug Tips

```rust
// Check matched star count
println!("Matched {} / {} stars", result.num_inliers, result.matched_stars.len());

// Examine residuals
let max_residual = result.residuals.iter().cloned().fold(0.0, f64::max);
println!("Max residual: {:.2} px", max_residual);

// Verify transform is reasonable
let (tx, ty) = result.transform.translation();
let scale = result.transform.scale();
let rotation = result.transform.rotation_angle();
println!("Translation: ({:.1}, {:.1}) px", tx, ty);
println!("Scale: {:.4}", scale);
println!("Rotation: {:.2}°", rotation.to_degrees());
```
