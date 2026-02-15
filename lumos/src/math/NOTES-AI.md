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

Related (star_detection/centroid/):
  lm_optimizer.rs   - Generic Levenberg-Marquardt optimizer (N-parameter)
  linear_solver.rs  - Gaussian elimination with partial pivoting (NxN)
  gaussian_fit/     - 2D Gaussian model (6 params), AVX2/NEON SIMD
  moffat_fit/       - 2D Moffat model (5 or 6 params), AVX2/NEON SIMD
```

## Architecture

### Summation Pipeline

SIMD dispatch hierarchy on x86_64: AVX2 (len >= 8) > SSE4.1 (len >= 4) > scalar. On aarch64: NEON (len >= 4) > scalar. Feature detection via `common::cpu_features`.

All paths use a hybrid compensated summation strategy:
- **SIMD inner loop**: Kahan summation per-lane (branchless: 3 ops per element)
- **Horizontal reduction**: Neumaier summation across lanes (branching but only 4-8 elements)
- **Scalar remainder**: Neumaier for any elements past the last full SIMD chunk

This hybrid is an intentional design choice consistent with [Dmitruk 2023]. Kahan is branchless and ideal for SIMD; Neumaier handles the |addend| > |sum| case that Kahan misses, which matters during reduction when accumulating lanes of different magnitudes.

### Statistics Pipeline

Sigma-clipped statistics use a two-phase median strategy:
1. **Intermediate iterations**: `median_f32_fast` -- single quickselect using `partial_cmp` (~30% faster, no NaN handling, upper-middle for even-length)
2. **Final result**: `median_f32_mut` -- quickselect using `f32::total_cmp` with proper even-length averaging

Two allocation strategies share core logic via `sigma_clip_iteration()`:
- `sigma_clipped_median_mad` -- heap-allocated Vec deviations buffer (reusable)
- `sigma_clipped_median_mad_arrayvec` -- stack-allocated ArrayVec (zero heap)

### L-M Optimizer Architecture

Generic over N parameters via `LMModel<N>` trait. Models override `batch_build_normal_equations` and `batch_compute_chi2` with SIMD implementations. The optimizer itself is model-agnostic.

Linear system solution: Gaussian elimination with partial pivoting (GEPP) in `linear_solver.rs`. Stack-allocated `[[f64; N]; N]` arrays -- no heap allocation for the solve path.

## Current State (Correct)

### Compensated Summation

All sum paths achieve O(n * eps^2) precision:

| Path | Inner Loop | Horizontal Reduction | Remainder |
|------|-----------|---------------------|-----------|
| Scalar | Neumaier (branch per element) | N/A | N/A |
| SSE4.1 (4 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |
| AVX2 (8 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |
| NEON (4 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |

**Kahan SIMD formula**: `y = v - c; t = sum + y; c = (t - sum) - y; sum = t`. Standard textbook. Branchless: 3 SIMD ops per element.

**Neumaier scalar formula**: `t = sum + v; if |sum| >= |v| then c += (sum - t) + v; else c += (v - t) + sum; sum = t`. Standard improved Kahan-Babuska form. Handles |addend| > |sum| correctly.

**Horizontal reduction**: Separates sum lanes and compensation lanes before Neumaier-reducing. Compensation lanes are negated (`-c_arr[i]`) because Kahan's `c` tracks error to *subtract* while Neumaier's `c` tracks error to *add*. Sign convention verified correct.

**Weighted mean**: Dual Kahan accumulation (numerator v*w and denominator w) per SIMD lane. Returns 0.0 when total weight <= f32::EPSILON. Standard formula `sum(v_i * w_i) / sum(w_i)`.

**Comparison vs NumPy/Julia**: NumPy uses pairwise summation with error O(log(n) * eps), worse than Kahan/Neumaier's O(n * eps^2). For f32 (eps ~ 6e-8), n * eps^2 ~ 3.6e-15 * n. Even for n = 1M, error ~ 3.6e-9, well below f32 representable precision. Pairwise's advantage is fewer operations per element but worse error bounds. The Kahan/Neumaier choice is correct for astronomy data.

**Error bound table**:

| Algorithm | Worst-case error | Notes |
|-----------|-----------------|-------|
| Naive | O(n * eps) | Unacceptable for large n |
| Pairwise | O(log(n) * eps + n * eps^2) | NumPy's approach |
| Kahan | O(n * eps^2) | Used in SIMD inner loops |
| Neumaier | O(n * eps^2) | Handles |addend| > |sum| case |
| Klein (KBN) | O(n * eps^3) | Not needed for f32 |

### Median Computation

Uses `select_nth_unstable_by(mid, f32::total_cmp)` -- O(n) quickselect (introselect with median-of-medians fallback). O(n) worst-case guaranteed since Rust 1.76+ (PR #107522, Fast Deterministic Selection).

**Even-length median**: After quickselect for upper-middle at position `mid`, lower-middle found as max of left partition `data[..mid]`. O(n) total. Matches NumPy behavior.

**NaN safety**: `f32::total_cmp` sorts NaN to the end. In sigma clipping, NaN values get large absolute deviations and are clipped in the first iteration. In bare `median_f32_mut`, NaN will not be selected as median for arrays with >= 2 non-NaN values.

**Approximate median** (`median_f32_fast`): Single quickselect using `partial_cmp`. For even-length arrays, returns upper-middle only (bias <= half the gap between middle values). NaN-unsafe by design (`partial_cmp` with `unwrap_or(Equal)` treats NaN as equal to everything). Only used in intermediate sigma-clipping iterations where data is NaN-free; final result uses exact `median_f32_mut`.

### MAD (Median Absolute Deviation)

`MAD = median(|x_i - median(x)|)`, scaled to sigma by `MAD_TO_SIGMA = 1.4826022`.

**Constant verification**: The exact value is 1/Phi^{-1}(3/4) = 1/0.6744897... = 1.4826022185... The stored constant `1.4826022` is correct to f32 precision (7 significant digits). Matches R's `mad()` default, Astropy's `mad_std`, SciPy's `median_abs_deviation(scale=1.4826)`, and GNU Astronomy Utilities.

**Implementation**: `mad_f32_with_scratch` computes deviations into external scratch buffer, then median. `median_and_mad_f32_mut` computes median in-place, then replaces values with absolute deviations, then computes median of deviations. Both correct.

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

**Deviations buffer recomputation** (step 6): Critical correctness requirement. `median_f32_fast` performs a partial sort on the deviations buffer, destroying the index correspondence `deviations[i] <-> values[i]`. Without recomputation, the wrong values get clipped. Regression test: `test_sigma_clip_asymmetric_outliers`.

**Astropy comparison**:
- Astropy default: `maxiters=5`, `sigma=3.0`, `cenfunc='median'`, `stdfunc='std'`
- This implementation: caller-specified iterations, caller-specified kappa, `cenfunc=median`, `stdfunc=MAD`
- Convergence: "iterate until the last iteration clips nothing" -- matches Astropy exactly
- Using MAD instead of std-dev is more robust for contaminated data. Astropy recommends `stdfunc='mad_std'` for robust estimation. This is the better default for astronomy background estimation.

**Edge cases handled**: empty (0,0), single value (val,0), two values (len < 3 stops iteration), uniform (sigma=0 early exit).

### FWHM-to-Sigma Constant

`FWHM_TO_SIGMA = 2.354_82` stored as f32. Exact value: 2*sqrt(2*ln(2)) = 2.354820045030949... Correct to f32 precision. Matches Astropy's `gaussian_sigma_to_fwhm`. Roundtrip test confirms < 1e-6 error.

### 3x3 Matrix (DMat3)

Row-major f64 3x3 matrix for 2D homogeneous transforms.

**Determinant**: Standard 3x3 cofactor expansion. Verified: identity=1, singular=0, diagonal=product, row-swap=-1.

**Inverse**: Cofactor/adjugate method with singularity threshold `det.abs() < 1e-12`. For pixel-scale coordinates (values 0-10000) with f64 (eps ~ 1e-16), this threshold provides ample headroom. The cofactor method is exact for 3x3 (no iterative refinement needed). Only the final division by determinant introduces error, bounded by eps/|det|.

**Transform point**: Homogeneous divide by w. Debug assert for w near zero.

**Additional operations**: Frobenius distance from identity (`deviation_from_identity`), scalar multiplication, `Mul` trait for matrix-matrix, matrix-point, and matrix-scalar.

### Levenberg-Marquardt Optimizer

Generic N-parameter L-M optimizer in `star_detection/centroid/lm_optimizer.rs`.

**Damping strategy**: `H[i][i] *= (1 + lambda)` -- Marquardt's form `J^T J + lambda * diag(J^T J)`. Provides scale invariance. Standard modern formulation per Fletcher (1971), used by MINPACK, GSL, Ceres Solver.

**Lambda adjustment**: `lambda_down = 0.1` (multiply on success), `lambda_up = 10.0` (multiply on failure). Lambda cap at 1e10 prevents infinite loops. Standard factors.

**Convergence criteria**: Three conditions:
1. `max_delta < convergence_threshold` (1e-8 default) -- parameter changes small
2. `chi2_rel_change < 1e-10` -- objective function plateau (both improvement and non-improvement)
3. `position_convergence_threshold` -- early exit when only centroid position matters (default 0.0 = disabled; condition `abs < 0.0` is always false)

**Parameter evaporation**: Multiplicative damping gives *less* damping to insensitive parameters (small Hessian diagonal), which can cause parameter wander. Mitigated by `constrain()` method clamping parameters to valid ranges. Adequate for the small parameter spaces (5-6 params) used here.

**Linear solver**: GEPP in `linear_solver.rs`. Singularity threshold 1e-15. Stack-allocated arrays.

### Gaussian and Moffat Models

**Gaussian 2D** (6 params): All analytical Jacobian derivatives verified against textbook formulas. SIMD exp() uses Cephes polynomial with < 1e-12 relative error (~2 ULP for f64), verified against SLEEF's 1 ULP standard.

**Moffat Fixed-Beta** (5 params): Jacobian verified. PowStrategy optimization avoids `powf` for integer/half-integer beta values (common in astronomy: beta=2.5 default).

**Moffat Variable-Beta** (6 params): Additional `df/dbeta = -A * ln(u) * u^(-beta)` verified. Uses `(-beta * ln(u)).exp()` instead of `u.powf(-beta)`.

### SIMD Verification

All three SIMD backends (AVX2, SSE4.1, NEON) implement structurally identical algorithms with platform-specific intrinsics. Test coverage: SIMD-vs-scalar parity, boundary sizes (4, 8, 16, 32 and +/-1), catastrophic cancellation, f64 reference comparison. 28 tests in sum/, 37 tests in statistics/.

## Issues Found

### Documentation Inaccuracy: sum/README.md

`sum/README.md` states `weighted_mean_f32` is "scalar only" in the function table. This is incorrect -- all three SIMD backends (AVX2, SSE4.1, NEON) implement `weighted_mean_f32` with Kahan compensated accumulation and the dispatch function routes to them. The README should list AVX2, SSE4.1, NEON in the SIMD column for `weighted_mean_f32`.

### Code Duplication: Horizontal Reduction

The Neumaier horizontal reduction logic is duplicated in four places:

1. `avx2.rs::sum_f32` (inline, 8 lanes)
2. `avx2.rs::reduce_kahan_256` (function, 8 lanes)
3. `sse.rs::sum_f32` (inline, 4 lanes)
4. `sse.rs::reduce_kahan_128` (function, 4 lanes)
5. `neon.rs::sum_f32` (inline, 4 lanes)
6. `neon.rs::reduce_kahan_neon` (function, 4 lanes)

Within each backend, `sum_f32` duplicates the reduction code that also exists in the corresponding `reduce_kahan_*` function. The `sum_f32` functions could call `reduce_kahan_*` instead of inlining the same logic. This would reduce the surface area for bugs if the reduction ever needs to change.

Additionally, the Neumaier scalar remainder loop is duplicated across all three `sum_f32` functions and the weighted mean remainder loops. A shared helper could reduce this.

### Naive Summation in L-M Chi2

`LMModel::batch_compute_chi2` default implementation uses `.sum()` (naive f64 summation) for the sum of squared residuals. For typical stamp sizes (15x15=225 to 31x31=961 pixels), the naive f64 error bound is n * eps ~ 961 * 1.1e-16 ~ 1e-13, which is adequate for chi2 comparison purposes. Not a bug -- just a noted asymmetry with the compensated approach used everywhere else.

## Missing Features

### Currently Unnecessary

These are standard features found in reference implementations that are not currently needed:

1. **Asymmetric sigma clipping** (Astropy `sigma_lower`/`sigma_upper`): Useful for skewed contamination. Star light adds only positive outliers to background tiles, so clipping more aggressively on the high side could improve background estimates. Low priority -- symmetric clipping at 3-sigma is adequate for most cases.

2. **Biweight estimators** (Astropy `biweight_location`/`biweight_scale` with tuning constants c=6.0/c=9.0): Robust estimation without explicit outlier removal. Produces smoother estimates than sigma clipping. Used in photutils for background estimation. Low priority.

3. **Weighted median**: Not implemented. Would be needed for heteroscedastic data. Not currently needed.

4. **Percentile computation**: Could use quickselect. Not currently needed.

5. **Cholesky decomposition** for the L-M solver: Would be ~2x faster than GEPP for N=5-6 SPD systems and avoids pivoting entirely. The Cholesky factorization is the top choice for SPD systems per numerical analysis literature. Very low priority -- the absolute difference is negligible at these sizes.

6. **Trust-region L-M** (Ceres: Levenberg-Marquardt + Dogleg): More robust convergence. Dogleg solves one linear system per successful step vs L-M's potentially many. Overkill for PSF fitting with good initial estimates.

7. **Variance/standard deviation**: No standalone variance function. Not needed when MAD-based estimation is used throughout.

### Potential Value-Add

These could genuinely improve quality if implemented:

1. **Error estimates from L-M covariance**: The inverse of the final Hessian `(J^T J)^{-1}` gives parameter covariance. This would provide uncertainty estimates for fitted positions. Currently, only `converged` boolean and `chi2` are returned. Adding position uncertainty would improve downstream astrometric quality assessment.

2. **Reduced chi-squared test**: Computing chi2 / (n_data - n_params) and comparing against 1.0 would provide a goodness-of-fit metric beyond just chi2. Could flag bad fits (chi2_reduced >> 1) or overfitting (chi2_reduced << 1).

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
| Biweight estimators | No | Yes (location/scale/midvariance) |
| Implementation | Rust + SIMD | Python/C |

This implementation follows Astropy's recommended robust approach (`stdfunc='mad_std'`).

### vs GNU Scientific Library (GSL)

| Component | This Implementation | GSL |
|-----------|-------------------|-----|
| Summation | Kahan/Neumaier (SIMD) | Basic summation (no compensation) |
| Median | quickselect O(n) | `gsl_stats_median` (full sort O(n log n)) |
| L-M optimizer | Normal equations + GEPP | Trust region + QR (more robust) |
| Linear solver | GEPP (NxN) | LU decomposition with pivoting |

The summation here is strictly better than GSL's basic approach. The L-M optimizer uses the simpler normal equations vs GSL's trust-region with QR. For well-conditioned PSF fitting problems (5-6 parameters, data SNR > 10), both converge to the same solution.

### vs Ceres Solver

| Component | This Implementation | Ceres |
|-----------|-------------------|-------|
| L-M damping | Marquardt diagonal scaling | Same (LEVENBERG_MARQUARDT) |
| Trust region | No | Dogleg option |
| Linear solver | GEPP | Dense Cholesky, Sparse QR, CGNR |
| Geodesic acceleration | No | Available |
| Covariance estimation | No | Yes (inverse Hessian) |

Ceres is a full-featured solver; this implementation covers the subset needed for PSF fitting.

### vs NumPy/SciPy

| Component | This Implementation | NumPy/SciPy |
|-----------|-------------------|-------------|
| Summation | Kahan/Neumaier (f32 SIMD) | Pairwise (weaker bounds) |
| Median | quickselect with total_cmp | introselect |
| MAD scaling | 1.4826022 (f32) | 1.4826 (SciPy) |

### vs PixInsight

| Feature | This Implementation | PixInsight |
|---------|-------------------|-----------|
| Sigma clipping | Standard rejection | Standard + Winsorized |
| Background estimation | Per-tile sigma clip | DBE (spline interpolation) |
| PSF model | Gaussian/Moffat | Gaussian/Moffat (same) |
| Integration rejection | N/A | Sigma clip + percentile + linear fit |

PixInsight's advanced features (Winsorized, percentile, linear fit clipping) are for image stacking, not background estimation.

## Recommendations

### No Code Changes Needed

The implementation is correct throughout. All algorithms match reference implementations (Astropy, GSL, Numerical Recipes). No mathematical errors or numerical stability issues found. Specific positive findings:

1. Compensated summation is textbook-correct with appropriate Kahan/Neumaier hybrid for SIMD
2. Sigma clipping correctly handles the deviations-recomputation issue (with regression test)
3. L-M damping strategy is standard Marquardt diagonal scaling with proper constraints
4. Gaussian and Moffat Jacobians are analytically correct
5. SIMD exp() approximation verified to < 1e-12 relative error
6. All constants (MAD_TO_SIGMA, FWHM_TO_SIGMA) verified against reference values
7. Test coverage is comprehensive: SIMD-vs-scalar parity, boundary conditions, precision tests with f64 references

### Minor Cleanup Opportunities

1. **Fix sum/README.md**: Update the `weighted_mean_f32` row to show SIMD support
2. **Deduplicate reduction code**: The horizontal reduction in `sum_f32` could call the corresponding `reduce_kahan_*` function instead of inlining identical logic. Low priority since all copies are verified correct.

### Architecture Notes

- The hybrid Kahan (SIMD) + Neumaier (scalar/reduction) approach is well-justified and matches [Dmitruk 2023]. Not unnecessary complexity.
- The PowStrategy optimization for Moffat is a clean pattern for avoiding per-pixel branching.
- Stack-allocated ArrayVec variants for hot loops are a good zero-allocation pattern.
- f64 throughout the fitting pipeline is the correct choice for numerical stability.
- `select_nth_unstable` now guarantees O(n) worst-case (Rust 1.76+), so no complexity concerns remain.

## References

- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
- [Astropy sigma_clipped_stats](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clipped_stats.html)
- [Astropy robust estimators](https://docs.astropy.org/en/stable/stats/robust.html) -- biweight, MAD, sigma clip
- [Astropy biweight_location](https://docs.astropy.org/en/stable/api/astropy.stats.biweight.biweight_location.html)
- [Astropy biweight_scale](https://docs.astropy.org/en/stable/api/astropy.stats.biweight.biweight_scale.html)
- [Astropy Moffat2D](https://docs.astropy.org/en/stable/api/astropy.modeling.functional_models.Moffat2D.html)
- [GNU Astronomy Utilities sigma clipping](https://www.gnu.org/software/gnuastro/manual/html_node/Sigma-clipping.html)
- [GNU Astronomy Utilities MAD clipping](https://www.gnu.org/software/gnuastro/manual/html_node/MAD-clipping.html)
- [Kahan summation - Wikipedia](https://en.wikipedia.org/wiki/Kahan_summation_algorithm)
- [Pairwise summation - Wikipedia](https://en.wikipedia.org/wiki/Pairwise_summation)
- [MAD - Wikipedia](https://en.wikipedia.org/wiki/Median_absolute_deviation)
- [MAD caveats (Akinshin)](https://aakinshin.net/posts/mad-caveats/)
- [Unbiased MAD (Akinshin)](https://aakinshin.net/posts/unbiased-mad/)
- [FWHM - Wikipedia](https://en.wikipedia.org/wiki/Full_width_at_half_maximum)
- [Levenberg-Marquardt - Wikipedia](https://en.wikipedia.org/wiki/Levenberg%E2%80%93Marquardt_algorithm)
- [Ceres Solver -- nonlinear least squares](http://ceres-solver.org/nnls_solving.html)
- [GSL nonlinear least-squares](https://www.gnu.org/software/gsl/doc/html/nls.html)
- [Ceres L-M strategy source](https://github.com/ceres-solver/ceres-solver/blob/master/internal/ceres/levenberg_marquardt_strategy.cc)
- [L-M improvements (Transtrum 2012)](https://arxiv.org/pdf/1201.5885) -- parameter evaporation discussion
- [L-M reference implementation (Gavin, Duke)](https://people.duke.edu/~hpgavin/lm.pdf)
- [L-M covariance estimation (MathWorks)](https://www.mathworks.com/matlabcentral/answers/252736)
- [Taming float sums (orlp.net)](https://orlp.net/blog/taming-float-sums/)
- [Fast, accurate summation (Bjornson)](http://blog.zachbjornson.com/2019/08/11/fast-float-summation.html)
- [Parallel vectorized Kahan/Gill-Moller (Dmitruk 2023)](https://onlinelibrary.wiley.com/doi/10.1002/cpe.7763)
- [SIMD pairwise sums (ACM 2014)](https://dl.acm.org/doi/10.1145/2568058.2568070)
- [Cholesky decomposition - Wikipedia](https://en.wikipedia.org/wiki/Cholesky_decomposition)
- [Cholesky vs GEPP for SPD (Cornell)](https://www.cs.cornell.edu/courses/cs4220/2022sp/lec/2022-02-16.pdf)
- [Cephes math library](http://www.netlib.org/cephes/)
- [SLEEF vectorized math](https://arxiv.org/pdf/2001.09258)
- [Moffat PSF determination (Bendinelli 1988)](https://adsabs.harvard.edu/full/1988JApA....9...17B)
- [photutils MoffatPSF](https://photutils.readthedocs.io/en/latest/api/photutils.psf.MoffatPSF.html)
- [photutils background estimation](https://photutils.readthedocs.io/en/stable/user_guide/background.html)
- [Rust select_nth_unstable O(n^2) fix (PR #107522)](https://github.com/rust-lang/rust/pull/107522)
- [Rust select_nth_unstable O(n^2) issue (#102451)](https://github.com/rust-lang/rust/issues/102451)
- [Matrix inversion stability (Higham, SIAM)](https://epubs.siam.org/doi/10.1137/1.9780898718027.ch14)
- [SciPy MAD](https://docs.scipy.org/doc//scipy-1.7.0/reference/generated/scipy.stats.median_absolute_deviation.html)
- [R mad() function](https://www.rdocumentation.org/packages/stats/versions/3.6.2/topics/mad)
