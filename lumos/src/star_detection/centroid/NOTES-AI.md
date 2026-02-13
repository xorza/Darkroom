# Centroid / PSF Fitting Submodule

## Overview

Three-tier centroid system for sub-pixel star position measurement:
1. **WeightedMoments** - Iterative Gaussian-weighted centroid (~0.05 px, fast)
2. **GaussianFit** - 6-parameter 2D Gaussian L-M fitting (~0.01 px)
3. **MoffatFit** - 5/6-parameter 2D Moffat L-M fitting (~0.01 px)

All fitting uses f64 arithmetic. Accuracy claims consistent with literature.

## File Map

| File | Purpose |
|------|---------|
| `mod.rs` | Dispatcher: stamp extraction, weighted moments, metrics, SNR |
| `lm_optimizer.rs` | Generic Levenberg-Marquardt with `LMModel<N>` trait |
| `linear_solver.rs` | Gaussian elimination with partial pivoting (NxN) |
| `gaussian_fit/mod.rs` | `Gaussian2D` (N=6), `fit_gaussian_2d` entry point |
| `gaussian_fit/simd_avx2.rs` | AVX2+FMA: Cephes exp(), 28 accumulators |
| `gaussian_fit/simd_neon.rs` | NEON: same algorithm, 2-wide f64 |
| `moffat_fit/mod.rs` | `MoffatFixedBeta` (N=5), `MoffatVariableBeta` (N=6), PowStrategy |
| `moffat_fit/simd_avx2.rs` | AVX2+FMA: 21 accumulators, SIMD int_pow/sqrt |
| `moffat_fit/simd_neon.rs` | NEON: same algorithm, 2-wide f64 |
| `test_utils.rs` | `add_noise`, `approx_eq`, `compute_hessian_gradient` |
| `tests.rs` | ~1500 lines of integration tests |

## Pipeline (mod.rs:322-440)

```
measure_star -> Phase 1: Weighted Moments (10 or 2 iters, 0.0001 px threshold)
             -> Local Background (GlobalMap or LocalAnnulus with sigma-clipped MAD)
             -> Phase 2: Profile Fitting (GaussianFit/MoffatFit via L-M)
             -> Metrics (flux, FWHM, eccentricity, SNR, sharpness, roundness)
             -> L.A.Cosmic Laplacian SNR
```

## Levenberg-Marquardt Optimizer (lm_optimizer.rs)

**Config** (lines 9-27): max_iterations=50, convergence=1e-8, initial_lambda=0.001,
lambda_up=10, lambda_down=0.1, position_convergence=0.0 (disabled).

**Damping** (lines 159-162): Marquardt's multiplicative form `H[i][i] *= (1+lambda)`.
Correctly blends Gauss-Newton (lambda=0) with gradient descent (large lambda), with
automatic per-parameter scaling from J^T J diagonal. This is the standard modern
formulation (Wikipedia, Numerical Recipes). The classic Levenberg additive form
(`H[i][i] += lambda`) does not respect parameter scaling.

**Convergence** (lines 176-206): Three exit conditions -- (1) max_delta < 1e-8,
(2) chi2_rel_change < 1e-10, (3) position-only convergence for centroid use cases.
Lambda capped at 1e10 to prevent infinite looping. Stall detection on rejected steps.

**Normal equations** (LMModel trait, lines 76-128): Fused build of J^T J, J^T r, and
chi2 in a single pass, avoiding NxM Jacobian storage. Separate `batch_compute_chi2`
for trial-step acceptance. Both overridable with SIMD.

**Linear solver** (linear_solver.rs:20-66): Gaussian elimination with partial pivoting.
Adequate for N=5-6 (Cholesky would be an alternative but negligible difference).

## Gaussian 2D Model (gaussian_fit/)

**6 parameters**: `[x0, y0, amplitude, sigma_x, sigma_y, background]`
Model: `A * exp(-0.5*(dx^2/sx^2 + dy^2/sy^2)) + B`

**Constraints** (mod.rs:112-116): amplitude >= 0.01, sigma in [0.5, stamp_radius].

**No rotation angle**: SExtractor fits 7 params (with theta). Missing theta is noted
as P3 in parent NOTES-AI.md. Negligible for near-circular ground-based PSFs.

**Initial estimates** (mod.rs:220-231): Position from Phase 1, amplitude from
peak-background, sigma from weighted second moments (`sigma = sqrt(E[r^2]/2)`).
Same approach as DAOPHOT GCNTRD and photutils centroid_2dg.

**SIMD** (simd_avx2.rs): 28 AVX2 accumulators (21 Hessian + 6 gradient + 1 chi2).
Fast Cephes exp() polynomial (~1e-13 accuracy, lines 46-101): range reduction via
`n = round(x*log2e)`, rational polynomial for exp(r), IEEE 754 bit manipulation for
2^n. Background Jacobian j5=1 optimized with vadd instead of vfmadd.
NEON version: identical algorithm, 2-wide f64 lanes.

## Moffat 2D Model (moffat_fit/)

**Model**: `A * (1 + r^2/alpha^2)^(-beta) + B`

**Fixed beta** (MoffatFixedBeta, N=5): `[x0, y0, amplitude, alpha, background]`.
Default beta=2.5 (ground-based seeing). Constraints: amplitude >= 0.01,
alpha in [0.5, stamp_radius].

**Variable beta** (MoffatVariableBeta, N=6): adds beta param, constrained [1.5, 10.0].

### Fixed vs Free Beta Tradeoff

Fixed beta is preferred for centroid use cases:
- 5 vs 6 parameters -> faster, more robust convergence
- Alpha-beta correlation causes degeneracy in 6-param fits
- SIMD PowStrategy only works for fixed beta (Int/HalfInt avoid powf)
- Centroid accuracy barely affected: wrong beta (2.5 vs 4.0) still <0.15 px
  (moffat_fit/tests.rs:318-354)
- Industry standard: Siril fixes beta=2.5; DAOPHOT uses empirical PSF

Free beta gives better FWHM accuracy when true beta differs significantly.

### PowStrategy (mod.rs:76-142)

Pre-selects computation at model construction:
- `Int { n }` -- integer beta: repeated squaring (lines 103-118)
- `HalfInt { int_part }` -- beta like 2.5: `1/(u^n * sqrt(u))` (lines 91-95)
- `General { neg_beta }` -- arbitrary: `u.powf(-beta)` (~10x slower)

SIMD versions (simd_avx2.rs:48-70) vectorize Int/HalfInt with `_mm256_sqrt_pd` and
`simd_int_pow`. General falls back to scalar powf per lane.

### FWHM (mod.rs:548-557)

`FWHM = 2 * alpha * sqrt(2^(1/beta) - 1)` -- standard Moffat 1969 formula.
Verified: alpha=2.0, beta=2.5 -> FWHM ~2.26 (tests.rs:126-133).

## Stamp Extraction (mod.rs:47-206)

**Radius**: `clamp(ceil(FWHM * 1.75), 4, 15)`. 1.75*FWHM captures ~99% of Gaussian
flux. Max 31x31 stamp (961 pixels). SExtractor uses similar adaptive aperture.
DAOPHOT FITRAD is typically ~1 FWHM.

**Stack allocation**: `ArrayVec<f32, 961>` avoids heap allocation. Three arrays
(x, y, z) plus peak value.

## Weighted Moments (mod.rs:446-512)

Iterative Gaussian-weighted first moment:
```
weight = max(pixel - bg, 0) * exp(-r^2 / (2*sigma^2))
centroid = sum(pos * weight) / sum(weight)
```

Sigma = `0.8 * FWHM / 2.355`, clamped [1.0, stamp_radius/2]. The 0.8 tightens
weighting. 10 iters standalone, 2 when fitting follows. Convergence: 0.0001 px.

Nearly identical to SExtractor XWIN/YWIN, whose docs state accuracy is "very close
to PSF-fitting on focused, properly sampled star images."

## Quality Metrics (mod.rs:537-757)

- **FWHM** (lines 618-620): `2.355 * sqrt(sum_r2/flux/2)`. Known P3 limitations:
  no pixel discretization correction, fit params discarded.
- **Eccentricity** (lines 622-637): Covariance eigenvalues, `e = sqrt(1-l2/l1)`.
- **SNR** (lines 728-757): Three models (full CCD / shot+sky / background-dominated).
  Known P2: uses full square stamp area as npix.
- **Sharpness** (lines 652-656): `peak/core_3x3`. Differs from DAOFIND.
- **Roundness** (lines 673-701): GROUND from marginal max, SROUND from marginal
  asymmetry. Both differ from DAOFIND (see parent NOTES-AI.md P2).

## Industry Comparison

### vs DAOPHOT (Stetson 1987)
DAOPHOT uses empirical PSF from bright stars and simultaneous multi-star fitting.
This implementation uses analytic models with single-star stamps. Both achieve ~0.01
px for well-sampled stars. Empirical PSF captures telescope aberrations but requires
more setup. Analytic models are adequate for image registration centroiding.

### vs SExtractor (Bertin & Arnouts 1996)
WeightedMoments is equivalent to XWIN/YWIN. Gaussian model lacks rotation angle
(6 vs 7 params). Background uses median vs SExtractor's mode estimator. SNR uses
square stamp vs optimal aperture.

### vs photutils (Python)
Custom L-M solver with SIMD vs scipy.optimize. AVX2 gives 4x f64 parallelism,
NEON gives 2x. Fused normal equations avoid Jacobian allocation.

### Achievable Accuracy (Literature)

| Method | Accuracy | Conditions |
|--------|----------|------------|
| Simple centroid | 0.1-0.3 px | Low SNR |
| Weighted moments / XWIN | 0.02-0.1 px | Well-sampled, moderate SNR |
| Gaussian/Moffat PSF fit | 0.01-0.05 px | SNR > 20 |
| Empirical PSF (DAOPHOT) | 0.005-0.02 px | High SNR, known PSF |

## Test Coverage

- **Gaussian** (28 tests): centered/offset/asymmetric, noise (5%/15%), edge cases,
  SIMD-vs-scalar validation, parameter boundaries, convergence with bad initial guess
- **Moffat** (29 tests): fixed/variable beta, various beta (1.5-6.0), wrong-beta
  centroid accuracy, PowStrategy correctness, SIMD validation across stamp sizes
- **Integration** (~80 tests): full measure_star pipeline, stamp validation, refine
  centroid convergence/rejection, compute_metrics scaling, Hessian properties

## Performance

- Stamps: 9x9 to 31x31. L-M: 5-15 iterations typical.
- AVX2 fused normal equations: ~47% faster than scalar
- Gaussian AVX2 with Cephes exp(): additional ~40-44%
- Total Gaussian SIMD: ~68-71% faster (25.8us -> 8.3us for 17x17)
- Zero heap allocation (ArrayVec for stamps and annulus)
