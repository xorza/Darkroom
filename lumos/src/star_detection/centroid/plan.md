# Centroid Improvements Plan

## Status Summary

| Phase | Description | Status |
|-------|-------------|--------|
| Phase 1 | Moment-based initial sigma estimate | ✅ Complete |
| Phase 2 | Adaptive weighted centroid σ | ✅ Complete |
| Phase 3 | Noise-weighted L-M optimization | ✅ Complete |
| Phase 4 | S-curve error handling | ⏸ Deferred |

---

## Completed Improvements

### Phase 1: Moment-Based Initial Sigma Estimate ✅

**Files modified:**
- `gaussian_fit.rs` - Added `estimate_sigma_from_moments()` helper
- `gaussian_fit.rs` - `fit_gaussian_2d()` now uses moment-based sigma estimate
- `moffat_fit.rs` - `fit_moffat_2d()` now uses moment-based alpha estimate (converted from sigma)

**Benefits:**
- Faster L-M convergence (fewer iterations)
- Better accuracy for PSFs that differ from σ=2.0 default
- More robust fitting across varying seeing conditions

### Phase 2: Adaptive Weighted Centroid σ ✅

**Files modified:**
- `mod.rs` - `refine_centroid()` now accepts `expected_fwhm` parameter
- `mod.rs` - `compute_centroid()` passes `config.expected_fwhm`
- `tests.rs` - Updated all test calls with `TEST_EXPECTED_FWHM`

**Implementation:**
```rust
// Adaptive sigma based on expected FWHM
// sigma ≈ FWHM / 2.355, use 0.8× for tighter weighting to reduce noise
let sigma = (expected_fwhm / 2.355 * 0.8).clamp(1.0, stamp_radius as f32 * 0.5);
```

**Benefits:**
- Better centroid accuracy across varying FWHM conditions
- Optimal weighting for both small and large PSFs

### Phase 3: Noise-Weighted L-M Optimization ✅

**Files modified:**
- `lm_optimizer.rs` - Added `optimize_5_weighted()` and `optimize_6_weighted()`
- `lm_optimizer.rs` - Added `compute_chi2_weighted()`, `compute_weighted_hessian_gradient_5/6()`
- `gaussian_fit.rs` - Added `compute_pixel_weights()` helper
- `gaussian_fit.rs` - Added `fit_gaussian_2d_weighted()` function
- `moffat_fit.rs` - Added `fit_moffat_2d_weighted()` function

**Weight calculation (CCD noise model):**
```rust
// variance = signal/gain + noise² + read_noise²/gain²
// weight = 1/variance
```

**Benefits:**
- Optimal estimation when noise characteristics are known
- Better accuracy in low-SNR regime
- Down-weights noisy pixels (wings, background)

---

## Deferred: Phase 4 - S-Curve Error Handling

**Reason for deferring:**
- S-curve error only affects undersampled PSFs (σ < ~0.85 pixels)
- Typical ground-based seeing has σ ~ 1.5-3 pixels → not affected
- Current implementation already uses Gaussian/Moffat fitting which doesn't have S-curve error
- Can be added later if needed for space telescope or short focal length applications

**Future implementation if needed:**
- Automatic fitting fallback when `expected_fwhm < 2.5` pixels
- Optional `correct_scurve()` helper for weighted centroid

---

## New Public API

### Weighted Fitting Functions

```rust
// Gaussian fitting with inverse-variance weighting
pub fn fit_gaussian_2d_weighted(
    pixels: &Buffer2<f32>,
    cx: f32, cy: f32,
    stamp_radius: usize,
    background: f32,
    noise: f32,
    gain: Option<f32>,
    read_noise: Option<f32>,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult>;

// Moffat fitting with inverse-variance weighting
pub fn fit_moffat_2d_weighted(
    pixels: &Buffer2<f32>,
    cx: f32, cy: f32,
    stamp_radius: usize,
    background: f32,
    noise: f32,
    gain: Option<f32>,
    read_noise: Option<f32>,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult>;
```

### Internal Helpers

```rust
// Estimate sigma from weighted second moments
pub(super) fn estimate_sigma_from_moments(...) -> f32;

// Compute inverse-variance weights from CCD noise model
pub(super) fn compute_pixel_weights(...) -> Vec<f32>;
```

---

## Test Results

All 81 centroid tests pass after implementation:
- Gaussian/Moffat fitting precision tests
- Weighted centroid convergence tests
- Quality metrics tests
- Integration tests

---

## References
- [S-curve centroiding error correction (ResearchGate)](https://www.researchgate.net/publication/261102712_S-curve_centroiding_error_correction_for_star_sensor)
- [Centroiding Undersampled PSFs with Lookup Table (arXiv)](https://arxiv.org/html/2407.04072v1)
- [Fast Gaussian Fitting for Star Sensors (PMC)](https://pmc.ncbi.nlm.nih.gov/articles/PMC6163372/)
