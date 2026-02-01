# Moffat PSF Fitting Optimization Plan

## Current Implementation Review

The current `moffat_fit.rs` implements 2D Moffat profile fitting using Levenberg-Marquardt optimization with the following features:

### Strengths
1. **Analytical Jacobians**: Correctly implements analytical derivatives for all parameters
2. **Two fitting modes**: Fixed beta (5 params) and variable beta (6 params)
3. **Weighted fitting**: Supports inverse-variance weighting for optimal estimation
4. **Buffer reuse**: L-M optimizer reuses jacobian/residual buffers across iterations
5. **Stack allocation**: Uses ArrayVec for stamp data extraction (no heap allocation)
6. **Parameter constraints**: Applies bounds to amplitude, alpha, and beta

### Current Limitations
1. **Circular PSF only**: No support for elliptical Moffat (rotation, axis ratio)
2. **Single-threaded**: No SIMD vectorization or parallel processing
3. **Fixed damping strategy**: Uses simple lambda up/down factors
4. **No geodesic acceleration**: Missing second-order convergence improvements
5. **Limited initial guess**: Relies on moment-based sigma estimation

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
- **Delayed gratification**: Increase lambda slowly (×1.5-2), decrease quickly (×3-5)
- **Adaptive damping**: Better convergence for highly coupled parameters

### Elliptical PSF Support

From [AsPyLib](http://www.aspylib.com/doc/aspylib_fitting.html) and [PampelMuse](https://pampelmuse.readthedocs.io/en/latest/psf.html):
- Elliptical Moffat requires 2 additional parameters: ellipticity (e) and position angle (θ)
- Off-axis optical aberrations make star shapes more elliptical
- Standard 8-parameter elliptical Moffat: x₀, y₀, amplitude, alpha, beta, background, ellipticity, theta

### Initial Guess Strategies

From [Stetson photometry guide](https://ned.ipac.caltech.edu/level5/Stetson/Stetson2_2_1.html):
- Initial guesses don't need to be optimal, just "vaguely all right"
- Peak value for amplitude, measured FWHM for alpha
- For variable beta, start with typical value (2.5-3.0)

---

## Optimization Priorities

### Priority 1: Improved Damping Strategy (Medium Impact, Low Effort)

**Problem**: Current fixed lambda_up=10, lambda_down=0.1 may be suboptimal.

**Solution**: Implement delayed gratification strategy.

```rust
// Current
lambda_up: 10.0,
lambda_down: 0.1,

// Improved (delayed gratification)
lambda_up: 2.0,    // Increase slowly
lambda_down: 0.33, // Decrease by factor of 3
```

**References**:
- [ResearchGate: Damping-undamping Strategies](https://www.researchgate.net/publication/242548995_Damping-undamping_Strategies_for_the_Levenberg-Marquardt_Nonlinear_Least_Squares_Method)

---

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

### Priority 3: Elliptical Moffat Support (High Impact, Medium Effort)

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

**References**:
- [AsPyLib elliptical Moffat](http://www.aspylib.com/doc/aspylib_fitting.html)
- [PampelMuse PSF documentation](https://pampelmuse.readthedocs.io/en/latest/psf.html)

---

### Priority 4: SIMD Vectorization of Jacobian (Medium Impact, Medium Effort)

**Problem**: Jacobian computation iterates over all stamp pixels serially.

**Solution**: Vectorize the inner loop using SIMD.

```rust
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

fn fill_jacobian_simd(
    data_x: &[f32],
    data_y: &[f32],
    params: &[f32; 5],
    jacobian: &mut [[f32; 5]],
) {
    // Process 8 pixels at a time with AVX2
    let chunks = data_x.len() / 8;
    for i in 0..chunks {
        // Load 8 x,y coordinates
        let x = _mm256_loadu_ps(&data_x[i * 8]);
        let y = _mm256_loadu_ps(&data_y[i * 8]);
        
        // Compute dx, dy, r2, u, etc. in parallel
        // ...
    }
}
```

**Expected improvement**: 4-8x speedup for Jacobian computation.

---

### Priority 5: Improved Initial Guess (Low Impact, Low Effort)

**Problem**: Moment-based sigma estimation may be poor for Moffat profiles.

**Solution**: Use 1D radial profile fitting for initial alpha/beta.

```rust
fn estimate_moffat_params_radial(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    cx: f32,
    cy: f32,
    background: f32,
) -> (f32, f32) {
    // Compute radial distances and values
    let mut radial: Vec<(f32, f32)> = data_x.iter()
        .zip(data_y.iter())
        .zip(data_z.iter())
        .map(|((&x, &y), &z)| {
            let r = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            (r, (z - background).max(0.0))
        })
        .collect();
    
    // Fit 1D Moffat to radial profile
    // ...
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

**Note**: Already may be implemented at detector level; verify before adding.

---

### Priority 7: Cached Power Computations (Low Impact, Low Effort)

**Problem**: `powf()` is expensive, called multiple times with same base.

**Solution**: Cache intermediate power values.

```rust
fn jacobian_row(&self, x: f32, y: f32, params: &[f32; 5]) -> [f32; 5] {
    let [x0, y0, amp, alpha, _bg] = *params;
    let alpha2 = alpha * alpha;
    let dx = x - x0;
    let dy = y - y0;
    let r2 = dx * dx + dy * dy;
    let u = 1.0 + r2 / alpha2;
    
    // Cache: compute ln(u) once, use for both powers
    let ln_u = u.ln();
    let u_neg_beta = (-self.beta * ln_u).exp();  // u^(-beta) = exp(-beta * ln(u))
    let u_neg_beta_m1 = u_neg_beta / u;          // u^(-beta-1) = u^(-beta) / u
    
    // ... rest of jacobian
}
```

---

## Implementation Order

| Phase | Priority | Description | Effort | Impact |
|-------|----------|-------------|--------|--------|
| 1 | P1 | Improved damping strategy | Low | Medium |
| 2 | P5 | Improved initial guess | Low | Low |
| 3 | P7 | Cached power computations | Low | Low |
| 4 | P2 | Geodesic acceleration | Medium | High |
| 5 | P3 | Elliptical Moffat support | Medium | High |
| 6 | P4 | SIMD vectorization | Medium | Medium |
| 7 | P6 | Parallel batch processing | Low | High |

---

## Benchmarking Strategy

Before and after each optimization:

```rust
#[quick_bench(warmup_iters = 10, iters = 100)]
fn bench_moffat_fit_single(b: ::bench::Bencher) {
    // Single Moffat fit timing
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_moffat_fit_batch_1000(b: ::bench::Bencher) {
    // 1000 stars batch timing
}

#[quick_bench(warmup_iters = 5, iters = 50)]
fn bench_moffat_convergence_iterations(b: ::bench::Bencher) {
    // Measure average iterations to convergence
}
```

---

## Expected Results

| Optimization | Current | Expected | Speedup |
|--------------|---------|----------|---------|
| Single Moffat fit | ~90µs | ~45µs | 2x |
| Batch 1000 (serial) | ~90ms | ~45ms | 2x |
| Batch 1000 (parallel, 8 cores) | ~90ms | ~8ms | 11x |
| Iterations to converge | ~20 | ~10 | 2x |

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
