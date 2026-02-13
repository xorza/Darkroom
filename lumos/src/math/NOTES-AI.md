# math Module

## Overview

Numerical primitives for astronomy image processing:
- **Summation**: SIMD-accelerated compensated summation (AVX2/SSE4.1/NEON), mean, weighted mean
- **Statistics**: Median (quickselect), MAD, iterative sigma-clipped rejection
- **Geometry**: 3x3 f64 matrix (projective transforms), axis-aligned bounding box, 2D usize vector
- **Constants**: FWHM-to-sigma conversion

## File Structure

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
  bbox.rs         - Axis-aligned bounding box (inclusive usize bounds), 8 tests
  vec2us.rs       - 2D usize vector, 7 tests
```

## Compensated Summation

### Architecture

All sum paths use compensated summation for O(n * eps^2) precision:

| Path | Inner Loop | Horizontal Reduction | Remainder |
|------|-----------|---------------------|-----------|
| Scalar | Neumaier (branch per element) | N/A | N/A |
| SSE4.1 (4 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |
| AVX2 (8 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |
| NEON (4 lanes) | Kahan per-lane (branchless) | Neumaier (sum + compensation separate) | Neumaier |

### Why Kahan for SIMD, Neumaier for Scalar

- **Kahan** inner loop: 3 ops (sub, add, sub) -- branchless, no comparison/blend needed in SIMD
- **Neumaier** requires `abs()` comparison per element -- would need SIMD `cmpgt + blendv` (extra ops)
- **Neumaier** for horizontal reduction: only 4-8 elements, branch cost negligible, better error bounds
- **Neumaier** for scalar: no SIMD overhead, strictly better than Kahan for alternating-magnitude data
- This hybrid is an intentional design: Kahan where branchless matters (SIMD hot loop), Neumaier
  where correctness matters more than throughput (reduction, scalar, remainder)

### Error Bounds (Theory)

| Algorithm | Worst-case error bound |
|-----------|----------------------|
| Naive | O(n * eps) |
| Pairwise | O(log(n) * eps + n * eps^2) |
| Kahan | O(n * eps^2) |
| Neumaier | O(n * eps^2), but handles |addend| > |sum| correctly |
| Klein (KBN) | O(n * eps^3) -- not implemented, diminishing returns for f32 |

For f32 (eps ~ 6e-8), n * eps^2 ~ 3.6e-15 * n. Even for n = 1M, error ~ 3.6e-9. This is
well below f32 representable precision, so O(n * eps^2) is effectively exact for f32.

### Correctness Assessment

**Verified correct**:
- Kahan SIMD formula: `y = v - c; t = sum + y; c = (t - sum) - y; sum = t` -- standard textbook
- Neumaier scalar formula: branch on `|sum| >= |v|` -- standard textbook
- Horizontal reduction correctly separates sum lanes and compensation lanes before Neumaier-reducing
- Compensation lanes are negated (`-c_arr[i]`) during reduction because Kahan's `c` tracks error
  to *subtract*, while Neumaier's `c` tracks error to *add* -- sign convention handled correctly
- Remainder elements use Neumaier -- correct
- `weighted_mean_f32`: dual Kahan accumulation (v*w and w) per SIMD lane, correct

**Potential issues**:
- None identified. The implementation is textbook-correct.

### Pairwise Summation: Not Used (Justified)

Pairwise summation (used by NumPy, Julia) has error O(log(n) * eps) -- worse than Kahan's O(n * eps^2).
For f32 astronomy data, Kahan/Neumaier is the right choice. Pairwise's advantage is fewer operations
(no compensation variable), but the 2.4x SIMD overhead is acceptable for astronomy precision needs.

### Benchmark Results (10k f32 elements, AVX2)

- Scalar Neumaier: ~13.2us
- SIMD Kahan (AVX2): ~3.8us (~3.5x speedup)
- Weighted mean scalar: ~24us
- Weighted mean AVX2: ~6us (~4x speedup)

## Statistics

### Median Computation

Uses `select_nth_unstable_by(mid, f32::total_cmp)` -- O(n) average quickselect.

**Even-length median**: After `select_nth_unstable_by(mid)`, the upper-middle element is at
position `mid`. The lower-middle element is the max of the left partition `data[..mid]`. This
is O(n) total (one quickselect + one linear scan). Standard and correct.

**NaN safety**: `f32::total_cmp` sorts NaN to the end. In sigma clipping, NaN values get
large absolute deviations and are clipped in the first iteration. In bare `median_f32_mut`,
NaN will not be selected as median for arrays with >= 2 non-NaN values.

**Approximate median** (`median_f32_approx`): Single `select_nth_unstable_by(mid)` call.
For even-length arrays, returns upper-middle only (bias <= half the gap between middle values).
Used only for intermediate sigma-clipping iterations; final result uses exact `median_f32_mut`.

**Known limitation**: Rust's `select_nth_unstable` is O(n^2) worst-case (issue #102451).
Not a practical concern: worst-case requires adversarial input; typical astronomy data is fine.

### MAD (Median Absolute Deviation)

`MAD = median(|x_i - median(x)|)`, scaled to sigma by `MAD_TO_SIGMA = 1.4826022`.

**Constant verification**: 1/Phi^{-1}(3/4) = 1/0.6744897... = 1.4826022... Correct to f32 precision.
GNU Astronomy Utilities uses the same constant (1/0.6745 ~ 1.4826).

**`mad_f32_with_scratch`**: Computes deviations into external scratch buffer, then median.
Avoids allocating on each call. Correct.

**`median_and_mad_f32_mut`**: Computes median, then replaces values with absolute deviations
in-place, then computes median of deviations. Correct and efficient (reuses buffer).

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

**Convergence criterion**: "No values clipped in this iteration." This matches Astropy's
default convergence criterion exactly. Astropy's `sigma_clip` also stops when "the last
iteration clips nothing." No relative-change threshold needed -- the standard approach.

**Astropy comparison**:
- Astropy default: `maxiters=5`, `sigma=3.0`, `cenfunc='median'`, `stdfunc='std'`
- This implementation: caller-specified iterations, caller-specified kappa, `cenfunc=median`, `stdfunc=MAD`
- Using MAD instead of std-dev is more robust (recommended by Astropy as `stdfunc='mad_std'`)
- No masked array support (not needed -- compacts in-place instead)

**Deviations buffer recomputation** (step 6): This was a bug fix. `median_f32_approx` on the
deviations buffer performs a partial sort, destroying the index correspondence `deviations[i] <-> values[i]`.
The recomputation is O(n) and necessary for correctness. Regression test: `test_sigma_clip_asymmetric_outliers`.

**Vec vs ArrayVec variants**: Both share `sigma_clip_iteration()` core. ArrayVec version avoids
heap allocation for small fixed-size data (e.g., per-tile loops). Parity verified by
`test_sigma_clipped_arrayvec_matches_vec_version`.

**Edge cases handled**: empty input (returns 0,0), single value (returns val,0), two values
(returns their mean, but < 3 stops iteration), uniform values (sigma=0 early exit).

### Weighted Mean

`weighted_mean_f32(values, weights)`: SIMD-dispatched with Kahan compensated dual accumulation
(numerator `sum(v*w)` and denominator `sum(w)`). Returns 0.0 if total weight <= f32::EPSILON.

**Correctness**: Standard formula `sum(v_i * w_i) / sum(w_i)`. Both sums use compensated
summation. Zero-weight check prevents division by zero.

**Note**: No weighted median is implemented. Not currently needed.

## DMat3 (3x3 Matrix)

Row-major f64 3x3 matrix for 2D homogeneous transforms.

### Operations

| Method | Description | Notes |
|--------|-------------|-------|
| `from_array`, `from_rows`, `identity` | Constructors | All `const` |
| `mul_mat` / `Mul<DMat3>` | Matrix multiply | Standard row-major formula |
| `determinant` | 3x3 cofactor expansion | Standard formula, correct |
| `inverse` | Cofactor/adjugate method | Returns `Option`, threshold 1e-12 |
| `transform_point` | Homogeneous 2D transform | Perspective divide by w |
| `deviation_from_identity` | Frobenius norm of (M - I) | For transform quality checks |
| `Mul<f64>`, `f64 * DMat3` | Scalar multiplication | Both directions |
| `Index`, `IndexMut` | Element access by flat index | Bounds-checked |

### Inverse: Singularity Threshold

Uses fixed threshold `det.abs() < 1e-12`. For pixel-scale coordinates (values 0-10000):
- Typical determinants for affine transforms: ~1.0 (rotation/scaling near identity)
- Typical determinants for projective transforms: can vary widely
- 1e-12 is appropriate for f64 with values in 0-10000 range

**Best practice comparison**: For general-purpose code, condition number is a better singularity
indicator than determinant magnitude (determinant scales with matrix entries). However, for this
specific use case (pixel-coordinate transforms with known value ranges), a fixed threshold is
acceptable and simpler. The f64 precision (eps ~ 1e-16) provides ample headroom.

### Correctness

- Matrix multiply: verified against known values and identity property
- Determinant: verified identity=1, singular=0, diagonal product, row-swap negation
- Inverse: verified roundtrip M * M^{-1} = I, diagonal inverse, singular returns None
- Transform point: verified identity, translation, perspective, roundtrip
- All formulas are standard textbook 3x3 cofactor expansion. No numerical stability concerns
  for f64 at pixel scales.

## Aabb (Bounding Box)

Axis-aligned bounding box with inclusive `usize` bounds.

### Design

- `min`/`max` are `Vec2us` (usize coordinates for direct array indexing)
- Inclusive bounds: pixel at `pos` is inside if `min <= pos <= max`
- `Aabb::empty()` uses inverted bounds (`min=MAX, max=0`) so first `include()` sets bounds

### Edge Cases

- **Empty box**: `is_empty()` checks `min.x > max.x || min.y > max.y` -- correct
- **Width/height of empty**: returns 0 -- correct (guarded by `is_empty()` check)
- **Single pixel**: `width()=1, height()=1, area()=1` -- correct (inclusive: `max-min+1`)
- **Merge of two empty**: produces empty (min of MAXes, max of 0s = still inverted)
- **usize underflow**: `Sub` for `Vec2us` will panic on underflow in debug, wrap in release.
  This is acceptable -- `Sub` on bbox coordinates should never underflow in correct usage.

### Missing (Not Needed)

- No intersection operation. Could be added if needed (`max of mins, min of maxes`).
- No `pad(margin)` operation. Could be added if needed.
- No iterator over contained points. Would be useful for some algorithms.

## Vec2us

2D vector with `usize` components for pixel coordinates.

- `to_index(width)` / `from_index(index, width)`: row-major layout conversion
- `Add`, `Sub`: component-wise (Sub panics on underflow in debug -- intentional)
- `From<(usize, usize)>` / `Into<(usize, usize)>`: tuple conversion

## What It Does Well

- MAD-based sigma clipping follows astronomy standards (Astropy `stdfunc='mad_std'` approach)
- Efficient O(n) median via Rust `select_nth_unstable_by` (quickselect)
- Approximate median optimization for intermediate sigma-clipping iterations
- SIMD dispatch hierarchy: AVX2 > SSE4.1 > scalar (x86_64); NEON always (aarch64)
- Zero-allocation ArrayVec variant for hot per-tile loops
- Hybrid Kahan (SIMD) + Neumaier (scalar/reduction) for optimal precision/throughput tradeoff
- NaN-safe median/sigma-clipping via `f32::total_cmp`
- Correct deviations recomputation after partial-sort destroys index correspondence
- Compensated summation for both plain sum and weighted mean (all SIMD backends)
- Good test coverage: precision tests with f64 references, catastrophic cancellation tests,
  SIMD boundary tests, SIMD-vs-scalar parity tests

## Potential Improvements (Not Currently Needed)

- **Biweight estimators**: Astropy provides `biweight_location` and `biweight_scale` for
  robust estimation without explicit outlier removal. More sophisticated than sigma clipping
  but heavier. Useful for galaxy cluster velocity dispersions; overkill for background estimation.
- **Trimmed mean**: Simple robust estimator (drop top/bottom k%). Not needed when sigma
  clipping already serves this purpose more adaptively.
- **Online/streaming statistics**: Welford's algorithm for single-pass mean/variance.
  Not needed -- all data is available in memory for our use cases.
- **Percentile computation**: Could add `percentile_f32_mut` using quickselect. Not currently needed.
- **Asymmetric sigma clipping**: Astropy supports `sigma_lower` != `sigma_upper`. Could be
  useful for skewed distributions (e.g., star contamination is one-sided). Not currently needed.

## References

- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html) -- convergence = "last iteration clips nothing"
- [Astropy robust estimators](https://docs.astropy.org/en/stable/stats/robust.html) -- biweight, MAD, sigma clip
- [GNU Astronomy Utilities sigma clipping](https://www.gnu.org/software/gnuastro/manual/html_node/Sigma-clipping.html)
- [GNU Astronomy Utilities MAD clipping](https://www.gnu.org/software/gnuastro/manual/html_node/MAD-clipping.html)
- [Kahan summation - Wikipedia](https://en.wikipedia.org/wiki/Kahan_summation_algorithm)
- [Pairwise summation - Wikipedia](https://en.wikipedia.org/wiki/Pairwise_summation)
- [Taming float sums (orlp.net)](https://orlp.net/blog/taming-float-sums/) -- error bound comparison, hybrid approach
- [Fast, accurate summation (blog)](http://blog.zachbjornson.com/2019/08/11/fast-float-summation.html)
- [Parallel vectorized Kahan/Gill-Moller (Dmitruk 2023)](https://onlinelibrary.wiley.com/doi/10.1002/cpe.7763)
- [SIMD pairwise sums (ACM 2014)](https://dl.acm.org/doi/10.1145/2568058.2568070)
- [Rust select_nth_unstable O(n^2) issue](https://github.com/rust-lang/rust/issues/102451)
- [MAD - Wikipedia](https://en.wikipedia.org/wiki/Median_absolute_deviation)
- [Matrix inversion stability (Higham, SIAM)](https://epubs.siam.org/doi/10.1137/1.9780898718027.ch14)
- [NIST GESD test](https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm)
