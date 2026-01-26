# Synthetic Test Plan for Image Registration

## Current Coverage

### transform_types.rs
Tests transform estimation from known star correspondences (no actual images):
- Translation only
- Euclidean (translation + rotation)
- Similarity (translation + rotation + scale)
- Affine (differential scaling, shear)
- Homography (perspective)
- Noise handling
- Large translations
- Cross-type validation (e.g., Similarity recovers Euclidean data)

### image_registration.rs
End-to-end tests with synthetic star field images:
- Translation recovery
- Rotation recovery
- Similarity transform recovery
- Noisy images
- Dense star fields
- Large images (1024x1024)

### warping.rs
Image warping roundtrip tests:
- All transform types
- All interpolation methods (Nearest, Bilinear, Bicubic, Lanczos2/3/4)
- Identity preservation
- Quality ordering verification
- End-to-end: detect -> register -> warp -> compare

---

## Robustness Tests (robustness.rs)

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
