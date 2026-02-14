# math Module

## Module Overview

Numerical primitives for astronomy image processing, organized into four areas:

- **Summation** (`sum/`): SIMD-accelerated compensated summation (AVX2/SSE4.1/NEON), mean, weighted mean
- **Statistics** (`statistics/`): Median (quickselect), MAD, iterative sigma-clipped rejection
- **Geometry**: 3x3 f64 matrix (`dmat3.rs`), axis-aligned bounding box (`bbox.rs`), 2D usize vector (`vec2us.rs`)
- **Constants**: FWHM-to-sigma conversion (`mod.rs`)

The module also supports the L-M optimizer and profile fitting code in `star_detection/centroid/`, which is analyzed here because it shares the mathematical infrastructure.

### File Structure

```
math/
  mod.rs          - Re-exports, FWHM_TO_SIGMA constant, fwhm_to_sigma/sigma_to_fwhm
  statistics/
    mod.rs        - median, MAD, sigma-clipped statistics (Vec + ArrayVec variants)
    tests.rs      - 37 tests: edge cases, NaN handling, regression tests, ArrayVec parity
    bench.rs      - Benchmarks: median (1k), sigma clip (1k, 4k tile)
    README.md     - Human-readable docs
  sum/
    mod.rs        - SIMD dispatch (AVX2 > SSE4.1 > NEON > scalar), mean_f32, weighted_mean_f32
    scalar.rs     - Neumaier compensated scalar sum + weighted mean
    sse.rs        - SSE4.1 Kahan sum + weighted mean, Neumaier horizontal reduction
    avx2.rs       - AVX2 Kahan sum + weighted mean, Neumaier horizontal reduction
    neon.rs       - NEON Kahan sum + weighted mean, Neumaier horizontal reduction
    tests.rs      - 28 tests: precision, SIMD boundaries, catastrophic cancellation, f64 refs
    bench.rs      - Scalar vs SSE vs AVX2 benchmarks (10k elements)
    README.md     - Human-readable docs
  dmat3.rs        - 3x3 f64 row-major matrix, 30 tests
  bbox.rs         - Axis-aligned bounding box (inclusive usize bounds), 14 tests
  vec2us.rs       - 2D usize vector, 11 tests

Related (star_detection/centroid/):
  lm_optimizer.rs   - Generic Levenberg-Marquardt optimizer (N-parameter)
  linear_solver.rs  - Gaussian elimination with partial pivoting (NxN)
  gaussian_fit/     - 2D Gaussian model (6 params), AVX2/NEON SIMD
  moffat_fit/       - 2D Moffat model (5 or 6 params), AVX2/NEON SIMD
```

## Algorithm Correctness Analysis

### Compensated Summation

All sum paths use compensated summation for O(n * eps^2) precision:

| Path | Inner Loop | Horizontal Reduction | Remainder |
|------|-----------|---------------------|-----------|
| Scalar | Neumaier (branch per element) | N/A | N/A |
| SSE4.1 (4 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |
| AVX2 (8 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |
| NEON (4 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |

**Kahan SIMD formula**: `y = v - c; t = sum + y; c = (t - sum) - y; sum = t` -- standard textbook (Wikipedia). Branchless: 3 SIMD ops per element. Correct.

**Neumaier scalar formula**: `t = sum + v; if |sum| >= |v| then c += (sum - t) + v; else c += (v - t) + sum; sum = t`. Standard improved Kahan-Babuska form. Handles |addend| > |sum| correctly, unlike base Kahan. Correct.

**Horizontal reduction**: Separates sum lanes and compensation lanes before Neumaier-reducing. Compensation lanes are negated (`-c_arr[i]`) because Kahan's `c` tracks error to *subtract* while Neumaier's `c` tracks error to *add*. Sign convention handled correctly.

**Weighted mean**: Dual Kahan accumulation (numerator v*w and denominator w) per SIMD lane. Returns 0.0 when total weight <= f32::EPSILON. Standard formula `sum(v_i * w_i) / sum(w_i)`. Correct.

**Why Kahan for SIMD, Neumaier for scalar**: Kahan's inner loop is branchless (3 ops: sub, add, sub) -- ideal for SIMD. Neumaier requires `abs()` comparison per element, which would need SIMD `cmpgt + blendv`. For horizontal reduction (4-8 elements), branch cost is negligible and Neumaier provides strictly better error bounds. This hybrid is an intentional and well-justified design choice, consistent with the approach described in [Dmitruk 2023](https://onlinelibrary.wiley.com/doi/10.1002/cpe.7763).

**Comparison vs NumPy/Julia**: NumPy uses pairwise summation with error O(log(n) * eps), which is worse than Kahan/Neumaier's O(n * eps^2). For f32 (eps ~ 6e-8), n * eps^2 ~ 3.6e-15 * n. Even for n = 1M, error ~ 3.6e-9, well below f32 representable precision. Kahan/Neumaier is the right choice for astronomy data where precision matters. Pairwise's advantage is fewer operations per element but worse error bounds.

### Median Computation

Uses `select_nth_unstable_by(mid, f32::total_cmp)` -- O(n) average quickselect. Matches the algorithm used by NumPy (`numpy.median` uses introselect) and SciPy.

**Even-length median**: After `select_nth_unstable_by(mid)`, the upper-middle element is at position `mid`. The lower-middle element is found as the max of the left partition `data[..mid]`. This is O(n) total (one quickselect + one linear scan). Standard and correct -- matches NumPy's behavior.

**NaN safety**: `f32::total_cmp` sorts NaN to the end. In sigma clipping, NaN values get large absolute deviations and are clipped in the first iteration. In bare `median_f32_mut`, NaN will not be selected as median for arrays with >= 2 non-NaN values. Correct behavior.

**Approximate median** (`median_f32_fast`): Single `select_nth_unstable_by(mid)` using `partial_cmp`. For even-length arrays, returns upper-middle only (bias <= half the gap between middle values). ~30% faster due to avoiding `total_cmp`. Used only for intermediate sigma-clipping iterations; final result uses exact `median_f32_mut`. Correct tradeoff.

**Known limitation**: Rust's `select_nth_unstable` is O(n^2) worst-case (issue #102451). Not a practical concern for astronomy data.

### MAD (Median Absolute Deviation)

`MAD = median(|x_i - median(x)|)`, scaled to sigma by `MAD_TO_SIGMA = 1.4826022`.

**Constant verification**: The exact value is 1/Phi^{-1}(3/4) = 1/0.6744897... = 1.4826022185... The stored constant `1.4826022` is correct to f32 precision (7 significant digits). This matches R's `mad()` default constant, Astropy's `mad_std`, and GNU Astronomy Utilities. SciPy also uses 1.4826.

**Implementation correctness**: `mad_f32_with_scratch` computes deviations into external scratch buffer, then median. `median_and_mad_f32_mut` computes median in-place, then replaces values with absolute deviations, then computes median of deviations. Both correct.

### Sigma-Clipped Median/MAD

Algorithm per iteration:
1. Compute approximate median (quickselect, upper-middle for even-length)
2. Copy values to deviations buffer, compute `|x_i - median|`
3. Compute approximate MAD from deviations (quickselect -- destroys index correspondence)
4. Convert MAD to sigma: `sigma = MAD * 1.4826022`
5. If sigma < eps: return (median, 0.0) -- all values identical
6. **Recompute deviations** from values (step 3 destroyed them via partial sort)
7. Clip values where `deviation[i] > kappa * sigma` (compact in-place)
8. If no values clipped: converged, return (median, sigma)
9. After all iterations: compute exact final median and sigma

**Astropy comparison**:
- Astropy default: `maxiters=5`, `sigma=3.0`, `cenfunc='median'`, `stdfunc='std'`
- This implementation: caller-specified iterations, caller-specified kappa, `cenfunc=median`, `stdfunc=MAD`
- Astropy convergence: "iterate until the last iteration clips nothing" -- matches this implementation exactly
- Using MAD instead of std-dev is more robust for contaminated data. Astropy recommends `stdfunc='mad_std'` for robust estimation. This is the better default for astronomy background estimation.
- No masked array support (not needed -- compacts in-place instead)

**PixInsight comparison**:
- PixInsight's sigma clipping uses similar iterative rejection but also offers "Winsorized" sigma clipping, where outliers are replaced with boundary values instead of removed. The standard clipping here (removal-based) is correct for background estimation. Winsorized clipping would only be needed for image stacking, which is not a current use case.

**Deviations buffer recomputation** (step 6): This was a critical bug fix. `median_f32_fast` performs a partial sort on the deviations buffer, destroying the index correspondence `deviations[i] <-> values[i]`. Without recomputation, the wrong values get clipped. Regression test: `test_sigma_clip_asymmetric_outliers`. The O(n) recomputation is necessary and correct.

**Edge cases handled**: empty input (returns 0,0), single value (returns val,0), two values (len < 3 stops iteration), uniform values (sigma=0 early exit).

### FWHM-to-Sigma Constant

`FWHM_TO_SIGMA = 2.354_82` stored as f32.

**Verification**: The exact value is 2*sqrt(2*ln(2)) = 2.354820045030949... Astropy stores this as `gaussian_sigma_to_fwhm = 2.3548200450309493` (f64). The stored f32 value `2.354_82` is correct to f32 precision. The roundtrip test confirms `sigma_to_fwhm(fwhm_to_sigma(x)) == x` within 1e-6. Correct.

### 3x3 Matrix (DMat3)

Row-major f64 3x3 matrix for 2D homogeneous transforms.

**Determinant**: Standard 3x3 cofactor expansion. Verified: identity=1, singular=0, diagonal=product, row-swap=-1. Correct.

**Inverse**: Cofactor/adjugate method with singularity threshold `det.abs() < 1e-12`. For pixel-scale coordinates (values 0-10000) with f64 (eps ~ 1e-16), this threshold provides ample headroom. Verified by roundtrip M * M^{-1} = I. For general-purpose code, condition number would be a better singularity indicator, but for this use case the fixed threshold is appropriate.

**Transform point**: Homogeneous divide by w. Debug assert for w near zero. Correct.

### Levenberg-Marquardt Optimizer

Generic N-parameter L-M optimizer in `star_detection/centroid/lm_optimizer.rs`.

**Damping strategy**: Uses `H[i][i] *= (1 + lambda)` -- this is Levenberg's additive damping scaled by the diagonal. Equivalent to `J^T J + lambda * diag(J^T J)` (Marquardt's form). This provides scale invariance: parameters with larger Hessian diagonals get proportionally more damping. This is the standard modern formulation per Fletcher (1971) and is used by most production implementations (MINPACK, GSL, Ceres Solver).

**Potential concern**: When a parameter is insensitive (small Hessian diagonal), multiplicative damping gives *less* damping, which can cause "parameter evaporation" (the parameter wanders to extreme values). The `constrain()` method mitigates this by clamping parameters to valid ranges after each step. This is adequate for the small parameter spaces (5-6 params) used here.

**Lambda adjustment**: `lambda_down = 0.1` (multiply on success), `lambda_up = 10.0` (multiply on failure). Standard factors. Lambda cap at 1e10 prevents infinite loops. Correct.

**Convergence criteria**: Three conditions checked:
1. `max_delta < convergence_threshold` (1e-8 default) -- parameter changes small
2. `chi2_rel_change < 1e-10` -- objective function plateau (both improvement and non-improvement)
3. `position_convergence_threshold` -- early exit when only centroid position matters

The dual chi2 check (both for improvement and failed steps) is sound -- it catches both convergence (small improvement) and stagnation (tiny failure).

**Linear solver**: Gaussian elimination with partial pivoting (GEPP) in `linear_solver.rs`. Singularity threshold 1e-15.

**Assessment vs Cholesky**: For L-M normal equations (J^T J is symmetric positive semi-definite after damping), Cholesky decomposition would be more efficient (n^3/3 vs 2n^3/3 flops) and requires no pivoting. However, for N=5 or N=6, the absolute performance difference is negligible (microseconds). GEPP is correct and numerically stable; Cholesky would be a micro-optimization.

**Comparison vs GSL/MINPACK**: GSL's `gsl_multifit_nlinear` uses a trust-region approach with more sophisticated damping (geodesic acceleration). MINPACK's `lmder` uses a similar basic L-M but with QR factorization instead of normal equations. The normal equations approach used here is standard and adequate for well-conditioned problems like PSF fitting. For ill-conditioned problems (not the case here), QR would be more stable.

### Gaussian 2D Model

`f(x,y) = A * exp(-0.5 * (dx^2/sx^2 + dy^2/sy^2)) + B`, 6 parameters: [x0, y0, A, sx, sy, B].

**Jacobian verification** (analytical derivatives):
- df/dx0 = A * exp_val * dx / sx^2 -- correct (chain rule: d/dx0 of -0.5*(x-x0)^2/sx^2 = dx/sx^2)
- df/dy0 = A * exp_val * dy / sy^2 -- correct (analogous)
- df/dA = exp_val -- correct (linear in A)
- df/dsx = A * exp_val * dx^2 / sx^3 -- correct (d/dsx of -0.5*dx^2/sx^2 = dx^2/sx^3)
- df/dsy = A * exp_val * dy^2 / sy^3 -- correct (analogous)
- df/dB = 1 -- correct (linear offset)

All derivatives verified against textbook formulas. The fused `evaluate_and_jacobian` shares the `exp_val` computation -- correct optimization.

**Constraints**: Amplitude > 0.01, sigma clamped to [0.5, stamp_radius]. These prevent degeneracy (zero amplitude, collapsed/exploded sigma). Appropriate for PSF fitting.

### Moffat Fixed-Beta Model

`f(x,y) = A * (1 + r^2/alpha^2)^(-beta) + B`, 5 parameters: [x0, y0, A, alpha, B].

**Jacobian verification**:
Let `u = 1 + r^2/alpha^2`, `pow = u^(-beta)`.
- df/dx0 = 2*A*beta/alpha^2 * u^(-beta-1) * dx -- correct (chain rule through u and pow)
- df/dy0 = 2*A*beta/alpha^2 * u^(-beta-1) * dy -- correct
- df/dA = u^(-beta) -- correct (linear in A)
- df/dalpha = 2*A*beta/alpha^2 * u^(-beta-1) * r^2/alpha -- correct (d/dalpha of -r^2/alpha^2 = 2r^2/alpha^3, combined)
- df/dB = 1 -- correct

The `common` factor `2*A*beta/alpha^2 * u^(-beta-1)` is shared across position and alpha derivatives. Efficient and correct.

**FWHM conversion**: `FWHM = 2 * alpha * sqrt(2^(1/beta) - 1)`. This is the standard formula. Verified: setting f(FWHM/2) = A/2 and solving gives this result. Correct.

**PowStrategy optimization**: Precomputes strategy at model construction. For beta=2.5 (common default), uses HalfInt path: `u^(-2.5) = 1/(u^2 * sqrt(u))`. For integer beta, uses integer power. For general beta, falls back to `powf`. The `select_pow_strategy` checks half-integer to within 1e-10 tolerance. Correct and significant speedup (avoids `powf` in hot loop).

### Moffat Variable-Beta Model

6 parameters: [x0, y0, A, alpha, beta, B]. Additional derivative:
- df/dbeta = -A * ln(u) * u^(-beta) -- correct (d/dbeta of u^(-beta) = -ln(u) * u^(-beta))

Uses `(-beta * ln(u)).exp()` instead of `u.powf(-beta)` for the exp+log path. This is numerically equivalent and avoids a separate `powf` implementation for the variable case. Correct.

**Beta constraints**: Clamped to [1.5, 10.0]. Physical range for atmospheric seeing is typically 2.5-4.5 (Moffat 1969 found 4.765 for pure turbulence). The wider range allows fitting unusual PSFs while preventing extreme values. Appropriate.

## Numerical Stability

### Compensated Summation Error Bounds

| Algorithm | Worst-case error bound | Notes |
|-----------|----------------------|-------|
| Naive | O(n * eps) | Unacceptable for large n |
| Pairwise | O(log(n) * eps + n * eps^2) | NumPy's approach |
| Kahan | O(n * eps^2) | Used in SIMD inner loops |
| Neumaier | O(n * eps^2) | Handles |addend| > |sum| correctly |
| Klein (KBN) | O(n * eps^3) | Not needed for f32 |

For f32 (eps ~ 6e-8), n * eps^2 ~ 3.6e-15 * n. Even for n = 1M, error ~ 3.6e-9, well below f32 representable precision (~1e-7 relative). Kahan/Neumaier is effectively exact for f32.

### L-M Optimizer Stability

- All fitting done in f64 throughout (data converted from f32 at entry). This eliminates catastrophic cancellation in residual computation and Hessian accumulation. Correct.
- Normal equations (J^T J) are symmetric but can be near-singular for poorly fitting models. The damping `H[i][i] *= (1+lambda)` guarantees positive definiteness when lambda > 0. With lambda_up = 10x, the system quickly becomes well-conditioned after failed steps.
- Chi2 relative change uses `prev_chi2.max(1e-30)` to prevent division by zero. Correct.

### SIMD exp() Approximation (Gaussian Fit)

The Cephes-derived polynomial exp() in `gaussian_fit/simd_avx2.rs` uses:
- Clamping to [-708, 709] (IEEE 754 f64 range)
- Range reduction via `n = round(x * log2(e))`, `r = x - n*ln2` using two-part ln2 (hi+lo) for precision
- Rational polynomial P(r^2)/Q(r^2) with Cephes coefficients (public domain, Stephen Moshier)
- IEEE 754 bit manipulation for 2^n reconstruction

**Accuracy**: Tests verify < 1e-12 relative error across [-700, 700]. This is ~2 ULP for f64. More than sufficient for L-M fitting which converges to ~1e-8.

**Comparison vs SLEEF/libm**: SLEEF (a production SIMD math library) achieves 1 ULP for exp(). The Cephes approximation at ~2 ULP is comparable and avoids an external dependency. Correct tradeoff.

### Moffat Power Strategy

For common beta values (integers and half-integers), `fast_pow_neg` avoids `powf` entirely:
- `u^(-2)` = `1 / (u*u)` -- 2 muls + 1 div, exact
- `u^(-2.5)` = `1 / (u^2 * sqrt(u))` -- 2 muls + 1 sqrt + 1 div, ~0.5 ULP
- General: `u.powf(-beta)` -- standard libm, ~1-2 ULP

All paths are numerically adequate. The integer and half-integer paths are actually more precise than `powf` for those special cases.

### Matrix Inverse Stability

The 3x3 inverse uses cofactor/adjugate method with threshold `det.abs() < 1e-12`. For f64 at pixel scales:
- Well-conditioned affine transforms: det ~ 1, condition number ~ 1-10. No stability concerns.
- Ill-conditioned projective transforms: det can be small. The 1e-12 threshold correctly rejects singular/near-singular matrices.
- The cofactor method is exact for 3x3 matrices (no iterative refinement needed). Only the final division by determinant introduces error, bounded by eps/|det|.

## SIMD Verification

### Sum Operations (sum/)

All SIMD backends (AVX2, SSE4.1, NEON) implement the same algorithm:
1. Kahan compensated summation in SIMD lanes
2. Neumaier horizontal reduction of sum + compensation lanes
3. Neumaier scalar tail for remainder elements

**AVX2** (`avx2.rs`): 8-lane f32, `_mm256_loadu_ps` + Kahan inner loop. `reduce_kahan_256` correctly stores 8 sum + 8 compensation values and Neumaier-reduces them. Weighted mean uses dual accumulation (v*w and w). Correct.

**SSE4.1** (`sse.rs`): 4-lane f32, `_mm_loadu_ps` + Kahan inner loop. Structurally identical to AVX2 at 4 lanes. `reduce_kahan_128` correct. Correct.

**NEON** (`neon.rs`): 4-lane f32, `vld1q_f32` + Kahan inner loop using `vsubq_f32/vaddq_f32`. Structurally identical to SSE path. `reduce_kahan_neon` correct. Correct.

**Dispatch**: AVX2 (len >= 8) > SSE4.1 (len >= 4) > scalar. NEON always on aarch64. Feature detection via `common::cpu_features`. Correct.

**Test coverage**: SIMD-vs-scalar parity, boundary sizes (4, 8, 16, 32 and +/-1), catastrophic cancellation, f64 reference comparison. 28 tests total. Comprehensive.

### Gaussian 2D Normal Equations (gaussian_fit/simd_avx2.rs)

Processes 4 f64 pixels per AVX2 iteration. 28 accumulators (21 Hessian upper triangle + 6 gradient + 1 chi2).

**Key operations verified**:
- `simd_exp_fast`: Cephes rational polynomial, verified < 1e-12 relative error
- Jacobian rows j0..j4 computed correctly from dx, dy, exp_val, inv_sigma terms
- j5 = 1.0 handled implicitly (gradient: add residual; Hessian: add j_k directly)
- Upper triangle accumulation: `h[i][j] += j[i] * j[j]` using FMA -- correct
- Scalar tail handles remainder (n % 4) pixels
- Mirror upper triangle to lower after accumulation

**Fits in 32 YMM registers**: 28 accumulators + ~4 temporaries = ~32 registers. On Skylake+ (32 YMM regs), no register spilling. Optimal.

### Moffat Fixed-Beta Normal Equations (moffat_fit/simd_avx2.rs)

Processes 4 f64 pixels per AVX2 iteration. 21 accumulators (15 Hessian + 5 gradient + 1 chi2).

**Key operations verified**:
- `simd_fast_pow_neg`: Dispatches on PowStrategy. HalfInt uses `simd_int_pow` + `_mm256_sqrt_pd`. Int uses `simd_int_pow`. General falls back to scalar `powf` per lane.
- `simd_int_pow`: Correct repeated squaring for n=0..5, general binary method for n>=6.
- Jacobian: `common = 2*amp*beta/alpha^2 * u^(-beta-1)`, with j0 = common*dx, j1 = common*dy, j2 = u_neg_beta, j3 = common*r^2/alpha. j4 = 1.0 implicit.
- All accumulation patterns match the scalar reference.

## Industry Comparison

### vs Astropy

| Feature | This Implementation | Astropy |
|---------|-------------------|---------|
| Sigma clip center | Median (quickselect) | Median (default) or mean |
| Sigma clip stdfunc | MAD * 1.4826 | std (default) or mad_std |
| Convergence | Last iteration clips nothing | Same |
| Default maxiters | Caller-specified | 5 |
| Default sigma | Caller-specified | 3.0 |
| Asymmetric clip | No | Yes (sigma_lower/sigma_upper) |
| Grow/shrink | No | Yes (grow parameter) |
| Masked arrays | No (compact in-place) | Yes |
| Implementation | Rust + SIMD | Python/C |

This implementation follows Astropy's recommended robust approach (`stdfunc='mad_std'`). The main missing feature is asymmetric clipping, which would be useful for one-sided contamination (e.g., star light only adds positive outliers to background).

### vs GNU Scientific Library (GSL)

| Component | This Implementation | GSL |
|-----------|-------------------|-----|
| Summation | Kahan/Neumaier (SIMD) | `gsl_sum_levin_u` (not compensated basic sum) |
| Median | quickselect O(n) | `gsl_stats_median` (full sort O(n log n)) |
| L-M optimizer | Normal equations + GEPP | Trust region + QR (more robust) |
| Linear solver | GEPP (NxN) | LU decomposition with pivoting |

The L-M optimizer here uses the simpler normal equations approach vs GSL's trust-region with QR factorization. For well-conditioned PSF fitting problems (5-6 parameters, data SNR > 10), both converge to the same solution. GSL's approach would be more robust for ill-conditioned problems.

### vs Numerical Recipes

| Algorithm | This Implementation | Numerical Recipes (3rd ed) |
|-----------|-------------------|---------------------------|
| Compensated sum | Kahan + Neumaier hybrid | Not specifically covered |
| Median | quickselect | quickselect (same approach) |
| L-M | Standard with diagonal damping | Standard with diagonal damping (same) |
| Profile fitting | Analytical Jacobian | Recommends analytical when available |

Consistent with Numerical Recipes recommendations.

### vs PixInsight

| Feature | This Implementation | PixInsight |
|---------|-------------------|-----------|
| Sigma clipping | Standard rejection | Standard + Winsorized |
| Background estimation | Per-tile sigma clip | DBE (spline interpolation) |
| PSF model | Gaussian/Moffat | Gaussian/Moffat (same) |
| Integration rejection | N/A | Sigma clip + percentile + linear fit |

PixInsight's more advanced features (Winsorized sigma clipping, percentile clipping, linear fit clipping) are for image stacking, not background estimation. For the current use case, standard sigma clipping is appropriate.

## Missing Features

### Currently Unnecessary

These are standard features found in reference implementations that are not currently needed:

1. **Asymmetric sigma clipping** (Astropy `sigma_lower`/`sigma_upper`): Useful for skewed contamination. Star light adds only positive outliers to background tiles, so clipping more aggressively on the high side could improve background estimates. Low priority -- symmetric clipping at 3-sigma is adequate for most cases.

2. **Biweight estimators** (Astropy `biweight_location`/`biweight_scale`): Robust estimation without explicit outlier removal. Produces smoother estimates than sigma clipping. Useful for precision photometry. Low priority.

3. **Weighted median**: Not implemented. Would be needed for heteroscedastic data (varying noise across pixels). Not currently needed.

4. **Percentile computation**: Could use quickselect. Not currently needed.

5. **Cholesky decomposition** for the L-M solver: Would be ~2x faster than GEPP for N=5-6 SPD systems and would avoid the need for pivoting. Very low priority -- the absolute difference is negligible at these sizes.

6. **Trust-region L-M** (a la GSL/Ceres): More robust convergence with geodesic acceleration. Overkill for PSF fitting with good initial estimates.

7. **Variance/standard deviation**: No standalone variance function. Not needed when MAD-based estimation is used throughout.

### Potential Value-Add

These could genuinely improve quality if implemented:

1. **Error estimates from L-M covariance**: The inverse of the final Hessian `(J^T J)^{-1}` gives parameter covariance. This would provide uncertainty estimates for fitted positions. Currently, only `converged` boolean and `rms_residual` are returned. Adding position uncertainty would improve downstream astrometric quality assessment.

2. **Reduced chi-squared test**: Computing chi2 / (n_data - n_params) and comparing against 1.0 would provide a goodness-of-fit metric beyond just RMS. Could flag bad fits (chi2_reduced >> 1) or overfitting (chi2_reduced << 1).

## Recommendations

### No Code Changes Needed

The implementation is correct throughout. All algorithms match reference implementations (Astropy, GSL, Numerical Recipes). No mathematical errors or numerical stability issues were found. Specific positive findings:

1. Compensated summation is textbook-correct with appropriate Kahan/Neumaier hybrid for SIMD
2. Sigma clipping correctly handles the deviations-recomputation issue (with regression test)
3. L-M damping strategy is standard Marquardt diagonal scaling with proper constraints
4. Gaussian and Moffat Jacobians are analytically correct
5. SIMD exp() approximation is verified to < 1e-12 relative error
6. All constants (MAD_TO_SIGMA, FWHM_TO_SIGMA) are verified against reference values
7. Test coverage is comprehensive: SIMD-vs-scalar parity, boundary conditions, precision tests with f64 references

### Architecture Notes

- The hybrid Kahan (SIMD) + Neumaier (scalar/reduction) approach is well-justified and matches the recommendation in [Dmitruk 2023]. Not unnecessary complexity -- it is the right design.
- The PowStrategy optimization for Moffat is a clean pattern for avoiding per-pixel branching.
- Stack-allocated ArrayVec variants for hot loops are a good zero-allocation pattern.
- f64 throughout the fitting pipeline is the correct choice for numerical stability.

## References

- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html) -- convergence = "last iteration clips nothing"
- [Astropy sigma_clipped_stats](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clipped_stats.html) -- combined statistics
- [Astropy robust estimators](https://docs.astropy.org/en/stable/stats/robust.html) -- biweight, MAD, sigma clip
- [Astropy Moffat2D](https://docs.astropy.org/en/stable/api/astropy.modeling.functional_models.Moffat2D.html) -- reference model
- [GNU Astronomy Utilities sigma clipping](https://www.gnu.org/software/gnuastro/manual/html_node/Sigma-clipping.html)
- [GNU Astronomy Utilities MAD clipping](https://www.gnu.org/software/gnuastro/manual/html_node/MAD-clipping.html)
- [Kahan summation - Wikipedia](https://en.wikipedia.org/wiki/Kahan_summation_algorithm)
- [Pairwise summation - Wikipedia](https://en.wikipedia.org/wiki/Pairwise_summation)
- [MAD - Wikipedia](https://en.wikipedia.org/wiki/Median_absolute_deviation)
- [Levenberg-Marquardt - Wikipedia](https://en.wikipedia.org/wiki/Levenberg%E2%80%93Marquardt_algorithm)
- [Taming float sums (orlp.net)](https://orlp.net/blog/taming-float-sums/) -- error bound comparison
- [Fast, accurate summation (Bjornson)](http://blog.zachbjornson.com/2019/08/11/fast-float-summation.html)
- [Parallel vectorized Kahan/Gill-Moller (Dmitruk 2023)](https://onlinelibrary.wiley.com/doi/10.1002/cpe.7763)
- [SIMD pairwise sums (ACM 2014)](https://dl.acm.org/doi/10.1145/2568058.2568070)
- [L-M improvements (Transtrum 2012)](https://arxiv.org/pdf/1201.5885)
- [L-M reference implementation (Gavin, Duke)](https://people.duke.edu/~hpgavin/lm.pdf)
- [Cholesky decomposition - Wikipedia](https://en.wikipedia.org/wiki/Cholesky_decomposition)
- [Cephes math library](http://www.netlib.org/cephes/) -- exp() polynomial coefficients
- [Moffat PSF determination (Bendinelli 1988)](https://adsabs.harvard.edu/full/1988JApA....9...17B)
- [photutils MoffatPSF](https://photutils.readthedocs.io/en/latest/api/photutils.psf.MoffatPSF.html)
- [Rust select_nth_unstable O(n^2) issue](https://github.com/rust-lang/rust/issues/102451)
- [Matrix inversion stability (Higham, SIAM)](https://epubs.siam.org/doi/10.1137/1.9780898718027.ch14)
- [NIST GESD test](https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm)
