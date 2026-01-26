# Synthetic Test Plan for Image Registration

## Overview

Comprehensive testing framework for image registration with 43 tests across 4 test suites. Tests validate transform estimation, full image pipelines, interpolation quality, and robustness to real-world challenges.

---

## Current Coverage

### transform_types.rs (~15 tests)
Tests transform estimation from known star correspondences (no actual images):
- Translation only (2 DOF)
- Euclidean (3 DOF): translation + rotation
- Similarity (4 DOF): translation + rotation + scale
- Affine (6 DOF): differential scaling, shear
- Homography (8 DOF): perspective
- Noise handling (±0.5 pixel coordinate noise)
- Large translations
- Cross-type validation (e.g., Similarity recovers Euclidean data with scale≈1.0)
- Transform property validation (min_points, degrees_of_freedom)

### image_registration.rs (~6 tests)
End-to-end tests with synthetic star field images:
- Translation recovery (±0.5 pixel accuracy)
- Rotation recovery (±0.02 rad accuracy)
- Similarity transform recovery
- Noisy images (0.04 noise sigma, adaptive detection thresholds)
- Dense star fields (200 stars)
- Large images (1024x1024, 50+ pixel translations)

### warping.rs (~8 tests)
Image warping roundtrip tests:
- All transform types (Translation, Euclidean, Similarity, Affine, Homography)
- All interpolation methods (Nearest, Bilinear, Bicubic, Lanczos2/3/4)
- Identity preservation (near-lossless)
- Quality ordering verification (Bilinear ≤ Bicubic ≤ Lanczos3)
- Quality metrics: PSNR (>40 dB excellent), NCC (>0.9), MSE
- End-to-end: detect -> register -> warp -> compare

---

## Robustness Tests (robustness.rs) (~14 tests)

All tests implemented and passing.

### 1. Outlier Rejection (RANSAC Robustness)
- [x] 10% spurious stars in target (false detections)
- [x] 10% missing stars in target (undetected real stars)
- [x] Combined: 10% spurious + 10% missing
- [x] 20% spurious stars (aggressive test)

### 2. Partial Overlap
- [x] 75% overlap (25% of stars missing from edges)
- [x] 50% overlap (half the field doesn't match)
- [x] Diagonal overlap (corner shift)

### 3. Subpixel Accuracy
- [x] 0.25 pixel translation recovery (within 0.1 px)
- [x] 0.5 pixel translation recovery
- [x] 0.1 degree rotation recovery (within 0.01 degrees)
- [x] 0.1% scale change recovery (within 0.05%)

### 4. Minimum Star Counts
- [x] 6 stars with translation
- [x] 8 stars with similarity transform
- [x] Graceful failure with insufficient stars (3 stars)

### 5. Combined Disturbances (Stress Tests)
- [x] Transform + noise + 10% missing + 5% spurious
- [x] 60% overlap + noise + rotation
- [x] Dense field (200 stars) + large translation + scale change

---

## Test Utilities

Located in `lumos/src/testing/synthetic/transforms.rs`:

```rust
/// Remove random stars from a list (simulate missed detections)
pub fn remove_random_stars(stars: &[(f64, f64)], fraction: f64, seed: u64) -> Vec<(f64, f64)>;

/// Add random spurious stars (simulate false detections)
pub fn add_spurious_stars(stars: &[(f64, f64)], count: usize, width: f64, height: f64, seed: u64) -> Vec<(f64, f64)>;

/// Filter stars to a bounding box (simulate partial overlap)
pub fn filter_to_bounds(stars: &[(f64, f64)], min_x: f64, max_x: f64, min_y: f64, max_y: f64) -> Vec<(f64, f64)>;

/// Translate stars and filter to bounds (simulate partial overlap with shift)
pub fn translate_with_overlap(stars: &[(f64, f64)], dx: f64, dy: f64, width: f64, height: f64, margin: f64) -> Vec<(f64, f64)>;

/// Add position noise to star coordinates
pub fn add_position_noise(stars: &[(f64, f64)], noise_amplitude: f64, seed: u64) -> Vec<(f64, f64)>;
```

---

## Suggestions for Future Improvements

### 1. Large-Scale Transform Coverage
Current tests cover moderate ranges. Consider adding:
- [ ] Very large translations (>500 pixels)
- [ ] Extreme scale changes (2x, 0.5x scale factors)
- [ ] Large rotations (45°, 90°)
- [ ] Stronger perspective distortion for Homography (h6/h7 > 0.0001)

### 2. Cross-Interpolation Method Tests
- [ ] Mixed interpolation (forward Lanczos, inverse Bilinear)
- [ ] Aliasing artifacts in undersampled regions
- [ ] Saturation handling with `clamp_output` variations
- [ ] Kernel normalization impact across methods

### 3. Detection Pipeline Stress Tests
- [ ] Variable SNR thresholds affecting star detection
- [ ] PSF width/elongation mismatches between generated and detected
- [ ] Star merging scenarios (blended stars detected as one)
- [ ] Non-Gaussian PSFs (Moffat profiles, saturated stars)

### 4. Full Affine & Homography Coverage
- [ ] Affine with all 6 DOF varying simultaneously (currently mostly scale+rotation)
- [ ] Realistic tilted-plane perspectives (simulate camera angle changes)
- [ ] Tests demonstrating when to prefer each transform type

### 5. WarpConfig Parameter Tests
- [ ] Different `border_value` impacts on roundtrip quality
- [ ] `clamp_output` effect on saturated regions
- [ ] Kernel normalization correctness validation

### 6. Performance Benchmarks
Consider adding `#[bench]` tests for:
- [ ] Execution time scaling with star count
- [ ] Transform type performance comparison
- [ ] Interpolation method throughput

### 7. Edge Case Seeds
- [ ] Near-degenerate star configurations (almost collinear)
- [ ] Boundary star distributions (clustered near edges)
- [ ] Document specific seed values that exercise edge cases

### 8. Failure Mode Documentation
Add explicit tests that document expected failures:
- [ ] Why exactly N stars is the minimum for each transform type
- [ ] Expected RMS thresholds per transform type
- [ ] RANSAC convergence behavior with varying star counts

---

## Strengths of Current Implementation

1. **Comprehensive Coverage**: 43 tests across 4 test suites
2. **Multi-Level Testing**: Unit -> component -> integration levels
3. **Deterministic**: All tests reproducible with fixed seeds (LCG PRNG)
4. **Realistic Scenarios**: Tests match actual astronomical use cases
5. **Quality Metrics**: Uses industry-standard PSNR/NCC/MSE
6. **Extensive Utilities**: Rich testing infrastructure in `testing/synthetic/`
7. **Parameterized Approaches**: Builder patterns, config structs for flexibility
