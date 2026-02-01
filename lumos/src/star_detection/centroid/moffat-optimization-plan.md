# Moffat PSF Fitting Optimization Plan

## Implementation Status

| Priority | Description | Status | Result |
|----------|-------------|--------|--------|
| P1 | Improved damping strategy | ❌ Reverted | Made performance worse (+54% slower) |
| P2 | Geodesic acceleration | ⏳ Pending | Medium effort, high impact |
| P3 | Elliptical Moffat support | ⏳ Pending | Feature addition, not perf |
| P4 | SIMD vectorization | ✅ Done | **73% speedup** (620ms → 155ms) |
| P5 | Improved initial guess | ⏳ Pending | Low effort, low impact |
| P6 | Parallel batch processing | ⏳ Pending | Check if already done at detector level |
| P7 | Cached power computations | ✅ Done | **15% speedup** (728ms → 620ms) |

### Benchmark Results (6K image, 10000 stars)

| Method | Baseline | After P7 | After P4 (SIMD) | Total Improvement |
|--------|----------|----------|-----------------|-------------------|
| weighted_moments | 29.8ms | 30.2ms | 30.2ms | — |
| gaussian_fit | 207.1ms | 213.4ms | 219.2ms | — |
| **moffat_fit** | **728.2ms** | **620.3ms** | **155.0ms** | **-78.7%** |

---

## Completed Optimizations

### P4: SIMD Vectorization ✅ (73% speedup)

Implemented AVX2 SIMD for Moffat fixed-beta optimization:

**Architecture**:
- `moffat_fit/simd/mod.rs` - Module exports and AVX2 detection
- `moffat_fit/simd/avx2.rs` - AVX2 SIMD implementations
- `moffat_fit/mod.rs` - SIMD-optimized L-M optimizer with fallback

**Key functions**:
- `compute_jacobian_residuals_8_fixed_beta()` - Processes 8 pixels in parallel
- `compute_chi2_8_fixed_beta()` - Computes chi² for 8 pixels
- `fill_jacobian_residuals_simd_fixed_beta()` - Wrapper with scalar fallback
- `compute_chi2_simd_fixed_beta()` - Wrapper with scalar fallback

**Approach**: Hybrid scalar+SIMD
- Uses **scalar `powf()`** for accuracy (L-M needs accurate gradients)
- Uses **SIMD (AVX2) for all other arithmetic** (dx, dy, r², divisions, multiplications)
- This gives accuracy of scalar with most of the SIMD benefit

**Convergence fix**: Added chi² stagnation detection for numerical precision limits:
```rust
// Check if chi2 is essentially the same (numerical precision limit)
let chi2_rel_diff = (new_chi2 - prev_chi2) / prev_chi2.max(1e-30);
if chi2_rel_diff < 1e-6 {
    converged = true;
    break;
}
```

### P7: Cached Power Computations ✅ (15% speedup)

Optimized Jacobian to compute `powf()` once and derive related values:

```rust
// Before: 2 expensive powf() calls
let u_neg_beta = u.powf(-self.beta);
let u_neg_beta_m1 = u.powf(-self.beta - 1.0);

// After: 1 powf() + 1 division
let u_neg_beta = u.powf(-self.beta);
let u_neg_beta_m1 = u_neg_beta / u;
```

For variable beta, use `ln()` + `exp()`:
```rust
let ln_u = u.ln();
let u_neg_beta = (-beta * ln_u).exp();
let u_neg_beta_m1 = u_neg_beta / u;
```

### P1: Delayed Gratification ❌ (Reverted)

Tested `lambda_up=2.0, lambda_down=0.33` but it caused 54% slower performance.
The algorithm took more iterations to escape bad regions with smaller lambda increases.
Original values (`lambda_up=10.0, lambda_down=0.1`) work better for this use case.

---

## Recommended Next Steps

### 1. **P6: Parallel Batch Processing** (Recommended Next)
- **Why**: Embarrassingly parallel, each star is independent
- **Effort**: Low - just add rayon
- **Expected**: Linear scaling with cores (8x on 8-core CPU)
- **Note**: First verify this isn't already done at detector level

### 2. **P2: Geodesic Acceleration** (Research needed)
- **Why**: Can reduce iterations by 2-5x for difficult cases
- **Effort**: Medium - requires second derivatives
- **Risk**: May not help much if current convergence is already fast

### 3. **Apply SIMD to Gaussian fitting**
- **Why**: Same approach should work for Gaussian fitting
- **Effort**: Low - copy SIMD pattern from Moffat
- **Expected**: Similar speedup (~3-4x)

---

## Current Implementation Review

The current `moffat_fit/` module implements 2D Moffat profile fitting using Levenberg-Marquardt optimization with the following features:

### Strengths
1. **Analytical Jacobians**: Correctly implements analytical derivatives for all parameters
2. **Two fitting modes**: Fixed beta (5 params) and variable beta (6 params)
3. **Weighted fitting**: Supports inverse-variance weighting for optimal estimation
4. **Buffer reuse**: L-M optimizer reuses jacobian/residual buffers across iterations
5. **Stack allocation**: Uses ArrayVec for stamp data extraction (no heap allocation)
6. **Parameter constraints**: Applies bounds to amplitude, alpha, and beta
7. **Cached power computations**: Single `powf()` call per pixel in Jacobian
8. **SIMD vectorization**: AVX2 SIMD for fixed-beta fitting (73% speedup)
9. **Numerical convergence detection**: Handles floating-point precision limits

### Remaining Limitations
1. **Circular PSF only**: No support for elliptical Moffat (rotation, axis ratio)
2. **Single-threaded batch**: No parallel processing of multiple stars
3. **Variable beta not SIMD**: Only fixed-beta mode uses SIMD optimization
4. **No geodesic acceleration**: Missing second-order convergence improvements

---

## Research Summary

### Moffat Profile Theory

From [Trujillo et al. (2001)](https://academic.oup.com/mnras/article/328/3/977/1247204):
- The Moffat function provides the best fit to atmospheric turbulence theory when **β ≈ 4.765**
- For seeing-limited observations, **β ≈ 2.5** is typical
- The Moffat PSF contains the Gaussian PSF as a limiting case (β → ∞)
- β must be > 1 to ensure finite integral (β = 1 gives Lorentzian with infinite integral)

### Numerical Stability

From [Astropy Moffat2D](https://docs.astropy.org/en/stable/api/astropy.modeling.functional_models.Moffat2D.html) and [Photutils MoffatPSF](https://photutils.readthedocs.io/en/latest/api/photutils.psf.MoffatPSF.html):
- Moffat functions are **numerically better behaved** than Gaussians for narrow PSFs
- Avoid exponential overflow issues that occur with Gaussians when σ < 1 pixel
- Default bounds: alpha > 0, beta > 1

### Levenberg-Marquardt Improvements

From [Wikipedia L-M](https://en.wikipedia.org/wiki/Levenberg%E2%80%93Marquardt_algorithm) and [arXiv:1201.5885](https://arxiv.org/pdf/1201.5885):
- **Geodesic acceleration**: Second-order correction for faster convergence in narrow canyons
- **Delayed gratification**: Increase lambda slowly (×1.5-2), decrease quickly (×3-5) — *did not help in our case*
- **Adaptive damping**: Better convergence for highly coupled parameters

---

## Pending Optimizations Detail

### Priority 2: Geodesic Acceleration (High Impact, Medium Effort)

**Problem**: Standard L-M converges slowly in parameter space canyons.

**Solution**: Add geodesic acceleration term to parameter update.

```rust
// Standard L-M step
delta = solve(H + lambda*diag(H), gradient)

// With geodesic acceleration
delta = solve(H + lambda*diag(H), gradient)
accel = compute_geodesic_acceleration(model, params, delta)
delta_improved = delta + 0.5 * accel
```

**Expected improvement**: 2-5x faster convergence for coupled parameters (x₀, y₀, alpha).

**References**:
- [arXiv:1201.5885 - Improvements to L-M](https://arxiv.org/pdf/1201.5885)

---

### Priority 3: Elliptical Moffat Support (Feature, Medium Effort)

**Problem**: Real PSFs are often elliptical due to tracking errors, field rotation, or optics.

**Solution**: Add 8-parameter elliptical Moffat model.

```rust
/// Elliptical Moffat model (8 parameters)
/// Parameters: [x0, y0, amplitude, alpha, beta, background, ellipticity, theta]
struct MoffatElliptical {
    stamp_radius: f32,
}

impl LMModel<8> for MoffatElliptical {
    fn evaluate(&self, x: f32, y: f32, params: &[f32; 8]) -> f32 {
        let [x0, y0, amp, alpha, beta, bg, e, theta] = *params;
        
        // Rotated coordinates
        let cos_t = theta.cos();
        let sin_t = theta.sin();
        let dx = x - x0;
        let dy = y - y0;
        let x_rot = dx * cos_t + dy * sin_t;
        let y_rot = -dx * sin_t + dy * cos_t;
        
        // Elliptical radius
        let r2 = x_rot * x_rot + y_rot * y_rot / ((1.0 - e) * (1.0 - e));
        
        amp * (1.0 + r2 / (alpha * alpha)).powf(-beta) + bg
    }
}
```

---

### Priority 6: Parallel Batch Processing (High Impact, Low Effort)

**Problem**: Processing many stars is embarrassingly parallel but done serially.

**Solution**: Use rayon for parallel centroid computation.

```rust
use rayon::prelude::*;

pub fn compute_centroids_parallel(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    candidates: &[StarCandidate],
    config: &StarDetectionConfig,
) -> Vec<Star> {
    candidates
        .par_iter()
        .filter_map(|c| compute_centroid(pixels, background, c, config))
        .collect()
}
```

**Note**: Check if already implemented at detector level before adding.

---

## References

1. [Siril Dynamic PSF](https://siril.readthedocs.io/en/latest/Dynamic-PSF.html) - Siril's L-M implementation with GSL
2. [Astropy Moffat2D](https://docs.astropy.org/en/stable/api/astropy.modeling.functional_models.Moffat2D.html) - Reference implementation
3. [Photutils MoffatPSF](https://photutils.readthedocs.io/en/latest/api/photutils.psf.MoffatPSF.html) - Jacobian computation
4. [arXiv:1201.5885](https://arxiv.org/pdf/1201.5885) - Geodesic acceleration for L-M
5. [Trujillo et al. 2001](https://academic.oup.com/mnras/article/328/3/977/1247204) - Moffat PSF theory
6. [Stetson Photometry](https://ned.ipac.caltech.edu/level5/Stetson/Stetson2_2_1.html) - Practical fitting techniques
7. [AsPyLib Fitting](http://www.aspylib.com/doc/aspylib_fitting.html) - Elliptical Moffat implementation
8. [PampelMuse PSF](https://pampelmuse.readthedocs.io/en/latest/psf.html) - Elliptical PSF support
