# Centroid Improvements Plan

## Overview
Four improvements to enhance centroid accuracy:
1. Make weighted centroid σ adaptive to expected FWHM
2. Use moment-based initial sigma estimate for fitting
3. Add noise-weighted chi² for L-M optimization
4. S-curve error correction for undersampled PSFs

---

## 1. Adaptive Weighted Centroid σ

### Current State
`refine_centroid()` in `mod.rs` uses hardcoded `sigma = 2.0`:
```rust
let sigma = 2.0f32; // Gaussian weight sigma
```

### Problem
- If expected FWHM is 6 pixels, σ=2.0 is too narrow (under-weights wings)
- If expected FWHM is 2 pixels, σ=2.0 is too wide (includes noise)

### Solution
Pass expected FWHM to `refine_centroid()` and compute adaptive sigma:
```rust
// sigma ≈ FWHM / 2.355 (Gaussian relationship)
// Use 0.8× for slightly tighter weighting to reduce noise
let sigma = (expected_fwhm / 2.355) * 0.8;
let sigma = sigma.clamp(1.0, stamp_radius as f32 * 0.5);
```

### Changes Required
1. **`refine_centroid()`** - Add `expected_fwhm: f32` parameter
2. **`compute_centroid()`** - Pass `config.expected_fwhm` to refine_centroid
3. **Tests** - Update test calls to include expected_fwhm

---

## 2. Moment-Based Initial Sigma Estimate

### Current State
`fit_gaussian_2d()` in `gaussian_fit.rs` uses fixed initial sigma:
```rust
let initial_params = [cx, cy, (peak_value - background).max(0.01), 2.0, 2.0, background];
```

Same in `fit_moffat_2d()` in `moffat_fit.rs`:
```rust
let initial_params = [cx, cy, (peak_value - background).max(0.01), 2.0, background];
```

### Problem
- If actual σ is 4.0, starting at 2.0 requires many L-M iterations
- Poor initial guess can lead to local minima

### Solution
Estimate sigma from weighted second moments before fitting:
```rust
fn estimate_sigma_from_moments(
    data_x: &[f32], data_y: &[f32], data_z: &[f32],
    cx: f32, cy: f32, background: f32
) -> f32 {
    let mut sum_r2 = 0.0f32;
    let mut sum_w = 0.0f32;
    
    for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
        let w = (z - background).max(0.0);
        let r2 = (x - cx).powi(2) + (y - cy).powi(2);
        sum_r2 += w * r2;
        sum_w += w;
    }
    
    if sum_w > f32::EPSILON {
        // For Gaussian: E[r²] = 2σ², so σ = sqrt(E[r²]/2)
        (sum_r2 / sum_w / 2.0).sqrt().clamp(0.5, 10.0)
    } else {
        2.0 // fallback
    }
}
```

### Changes Required
1. **`gaussian_fit.rs`** - Add `estimate_sigma_from_moments()` helper
2. **`gaussian_fit.rs`** - Use estimated sigma in `fit_gaussian_2d()`
3. **`moffat_fit.rs`** - Use estimated sigma (converted to alpha) in `fit_moffat_2d()`

---

## 3. Noise-Weighted Chi² for L-M Optimization

### Current State
`lm_optimizer.rs` uses uniform weights for chi²:
```rust
fn compute_chi2<const N: usize, M: LMModel<N>>(...) -> f32 {
    data_x.iter().zip(data_y.iter()).zip(data_z.iter())
        .map(|((&x, &y), &z)| {
            let residual = z - model.evaluate(x, y, params);
            residual * residual  // uniform weight
        })
        .sum()
}
```

### Problem
- All pixels weighted equally regardless of noise
- Low-SNR pixels (wings, background) have same influence as high-SNR pixels (core)
- Optimal estimation requires inverse-variance weighting

### Solution
Add optional noise weights to L-M optimizer:

```rust
// New weighted chi² computation
fn compute_chi2_weighted<const N: usize, M: LMModel<N>>(
    model: &M,
    data_x: &[f32], data_y: &[f32], data_z: &[f32],
    weights: Option<&[f32]>,  // inverse variance weights
    params: &[f32; N],
) -> f32 {
    match weights {
        Some(w) => {
            data_x.iter().zip(data_y.iter()).zip(data_z.iter()).zip(w.iter())
                .map(|(((&x, &y), &z), &weight)| {
                    let residual = z - model.evaluate(x, y, params);
                    weight * residual * residual
                })
                .sum()
        }
        None => {
            // Fallback to uniform weights
            compute_chi2(model, data_x, data_y, data_z, params)
        }
    }
}
```

Weight calculation from CCD noise model:
```rust
// For each pixel: variance = signal/gain + background_noise² + read_noise²/gain²
// Weight = 1/variance
fn compute_pixel_weights(
    data_z: &[f32],
    background: f32,
    noise: f32,
    gain: Option<f32>,
    read_noise: Option<f32>,
) -> Vec<f32> {
    data_z.iter().map(|&z| {
        let signal = (z - background).max(0.0);
        let variance = match (gain, read_noise) {
            (Some(g), Some(rn)) => signal / g + noise * noise + (rn * rn) / (g * g),
            (Some(g), None) => signal / g + noise * noise,
            _ => noise * noise + signal.max(1.0),  // Approximate Poisson
        };
        1.0 / variance.max(0.01)  // Avoid division by zero
    }).collect()
}
```

### Changes Required
1. **`lm_optimizer.rs`** - Add `weights: Option<&[f32]>` parameter to optimize functions
2. **`lm_optimizer.rs`** - Update chi² and gradient computations for weights
3. **`gaussian_fit.rs`** - Add noise/gain parameters, compute weights, pass to optimizer
4. **`moffat_fit.rs`** - Same as gaussian_fit
5. **`mod.rs`** - Pass gain/read_noise/noise to fitting functions

---

## 4. S-Curve Error Correction

### Background
S-curve error is a systematic centroid bias that occurs with undersampled PSFs (σ < ~0.85 pixels).
It manifests as a sinusoidal error pattern with period of 1 pixel.

**Key characteristics:**
- Affects Center-of-Gravity and Weighted Centroid methods
- Does NOT affect Gaussian/Moffat fitting methods
- Amplitude decreases as PSF sigma increases
- Error is ~sinusoidal: `error ≈ A × sin(2π × fractional_position)`

**Methods affected:**
| Method | Has S-curve error |
|--------|------------------|
| Center of Gravity | Yes |
| Weighted Centroid | Yes |
| Gaussian Fitting | No |
| Moffat Fitting | No |

### When It Matters
- σ < 0.85 pixels (undersampled, Nyquist criterion)
- Typical ground-based seeing: σ ~ 1.5-3 pixels → **not a problem**
- Space telescopes, short focal lengths: σ ~ 0.5-1.0 pixels → **may matter**

### Solution Options

#### Option A: Use Fitting for Undersampled PSFs (Recommended)
Since Gaussian/Moffat fitting doesn't exhibit S-curve error, automatically switch to
fitting when PSF is undersampled:

```rust
// In compute_centroid():
let use_fitting = config.expected_fwhm < 2.5; // σ < ~1.0 pixels
if use_fitting || matches!(config.centroid_method, CentroidMethod::GaussianFit | CentroidMethod::MoffatFit) {
    // Use fitting (no S-curve error)
} else {
    // Use weighted centroid (may have S-curve for small PSFs)
}
```

#### Option B: S-Curve Correction Lookup Table
For maximum accuracy with weighted centroid on undersampled PSFs:

```rust
/// Correct S-curve error for weighted centroid.
/// 
/// Based on Wei et al. 2014, the S-curve error is approximately sinusoidal:
/// error ≈ A(σ) × sin(2π × frac_pos)
/// 
/// where A(σ) is the amplitude that depends on PSF sigma.
fn correct_scurve(cx: f32, cy: f32, sigma: f32) -> (f32, f32) {
    // Amplitude decreases with sigma, negligible for σ > 1.5
    if sigma > 1.5 {
        return (cx, cy);
    }
    
    // Empirical amplitude formula (from literature)
    // A ≈ 0.1 × exp(-2 × σ) for σ < 1.5
    let amplitude = 0.1 * (-2.0 * sigma).exp();
    
    let frac_x = cx - cx.floor() - 0.5; // [-0.5, 0.5]
    let frac_y = cy - cy.floor() - 0.5;
    
    // S-curve correction (subtract estimated error)
    let correction_x = amplitude * (2.0 * std::f32::consts::PI * frac_x).sin();
    let correction_y = amplitude * (2.0 * std::f32::consts::PI * frac_y).sin();
    
    (cx - correction_x, cy - correction_y)
}
```

#### Option C: PSF-Specific Lookup Table (Most Accurate)
Pre-compute centroid errors for specific PSF at many sub-pixel positions,
then use nearest-neighbor lookup. This requires characterizing the actual PSF.

### Recommendation
**Use Option A** (automatic fitting for undersampled PSFs) as default behavior.
This is simpler and avoids S-curve error entirely rather than trying to correct it.

Option B can be added as an optimization for cases where fitting is too slow
but weighted centroid accuracy is needed for small PSFs.

### Changes Required
1. **`mod.rs`** - Add check for undersampled PSF in `compute_centroid()`
2. **`mod.rs`** - Optionally add `correct_scurve()` helper
3. **Tests** - Add test verifying S-curve behavior and correction

---

## Implementation Order

### Phase 1: Moment-based sigma estimate (no API changes)
1. Add `estimate_sigma_from_moments()` to `gaussian_fit.rs`
2. Update `fit_gaussian_2d()` to use moment-based estimate
3. Update `fit_moffat_2d()` to use moment-based estimate
4. Run tests

### Phase 2: Adaptive weighted centroid σ
5. Update `refine_centroid()` signature to accept `expected_fwhm`
6. Update `compute_centroid()` to pass expected_fwhm
7. Update tests
8. Run tests

### Phase 3: Noise-weighted L-M
9. Update `LMConfig` or add weights to optimize functions
10. Add weight computation helpers
11. Update `fit_gaussian_2d()` and `fit_moffat_2d()` signatures
12. Update callers to pass noise parameters
13. Run tests and linters

### Phase 4: S-curve handling
14. Add automatic fitting fallback for undersampled PSFs
15. Optionally add `correct_scurve()` helper
16. Add tests for S-curve behavior
17. Run tests and linters

---

## Expected Outcome
- **Phase 1**: Faster L-M convergence, fewer iterations for non-standard PSF sizes
- **Phase 2**: Better centroid accuracy across varying FWHM conditions
- **Phase 3**: Optimal weighting for noisy data, improved accuracy in low-SNR regime
- **Phase 4**: Eliminate S-curve systematic error for undersampled PSFs

---

## References
- [S-curve centroiding error correction (ResearchGate)](https://www.researchgate.net/publication/261102712_S-curve_centroiding_error_correction_for_star_sensor)
- [Centroiding Undersampled PSFs with Lookup Table (arXiv)](https://arxiv.org/html/2407.04072v1)
- [Fast Gaussian Fitting for Star Sensors (PMC)](https://pmc.ncbi.nlm.nih.gov/articles/PMC6163372/)
