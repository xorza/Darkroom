# Synthetic Tests for Image Registration

Comprehensive testing framework with 57 tests across 4 test suites validating transform estimation, image pipelines, interpolation quality, and robustness.

## Test Modules

| Module | Tests | Description |
|--------|-------|-------------|
| `transform_types.rs` | ~15 | Transform estimation from star correspondences |
| `image_registration.rs` | ~6 | End-to-end tests with synthetic images |
| `warping.rs` | ~8 | Interpolation roundtrip quality |
| `robustness.rs` | ~28 | Outliers, overlap, edge cases, stress tests |

## Coverage

**Transform Types:** Translation, Euclidean, Similarity, Affine, Homography

**Robustness Scenarios:**
- Outlier rejection (10-20% spurious/missing stars)
- Partial overlap (50-75%)
- Subpixel accuracy (0.1 px translation, 0.01° rotation)
- Large rotations (45°, 90°)
- Extreme scales (0.5x, 2x)
- Combined stress tests

**Quality Metrics:** PSNR, NCC, MSE

## Test Utilities

Located in `lumos/src/testing/synthetic/transforms.rs`:
- `remove_random_stars()` - simulate missed detections
- `add_spurious_stars()` - simulate false detections  
- `add_position_noise()` - add coordinate noise
- `translate_with_overlap()` - simulate partial overlap

All tests use deterministic seeded RNG for reproducibility.
