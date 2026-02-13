# math Module - Code Review vs Industry Standards

## Overview
Numerical primitives: SIMD-accelerated summation (AVX2/SSE4.1/NEON), mean, weighted mean,
robust statistics (median via quickselect, MAD, iterative sigma-clipped rejection),
3x3 f64 matrix for projective transforms, bounding box, 2D usize vector, FWHM-sigma conversions.

## What It Does Well
- MAD-based sigma clipping follows astronomy standards (matches Astropy/Siril approach)
- Efficient O(n) median via Rust's `select_nth_unstable_by` (quickselect)
- Approximate median optimization for intermediate sigma-clipping iterations (saves a second partial sort)
- SIMD dispatch hierarchy: AVX2 > SSE4.1 > scalar on x86_64; NEON always on aarch64
- Zero-allocation ArrayVec variant for hot per-tile loops
- Correct MAD_TO_SIGMA constant: 1.4826022 matches 1/inverse-CDF(3/4) for normal distribution
- Correct DMat3: cofactor inverse (documented pixel-scale threshold), perspective divide, Frobenius norm
- Good test coverage for statistics and SIMD boundaries
- NaN-safe median/sigma-clipping via `f32::total_cmp` (NaN sorts to end, gets clipped)
- `Aabb::is_empty()` with correct `width()`/`height()`/`area()` returning 0 for empty boxes
- `weighted_mean_f32` has `debug_assert_eq` on lengths, returns 0.0 on zero weights
- `transform_point` has `debug_assert` for w=0 (points at infinity)

## Remaining Issues (Performance Only)

### No Compensated Summation - Precision Loss for Large Arrays
- Files: `sum/scalar.rs`, `sum/sse.rs`, `sum/avx2.rs`, `sum/neon.rs`
- All use naive sequential accumulation: scalar uses `iter().sum()`, SIMD uses single
  accumulator vector per lane with naive horizontal reduction.
- **Error analysis**: Naive summation has O(n*eps) worst-case error. For a 4096-pixel
  tile of values around 100.0, f32 (23-bit mantissa, ~7 decimal digits) can lose 1-2
  significant digits in the sum.
- SIMD lanes provide partial pairwise benefit (4 or 8 independent accumulators), but the
  horizontal reduction at the end and the scalar remainder loop are still naive.
- **Industry standard**: Pairwise summation achieves O(log(n)*eps) with negligible cost.
  Neumaier/Kahan compensated summation achieves O(n*eps^2) -- essentially independent of n.
  Python 3.12+ stdlib uses Neumaier for `sum()`. NumPy uses pairwise summation.
- **Impact**: For the tile sizes used (64x64 = 4096), the error is tolerable but not optimal.
  For weighted mean (which sums products), errors compound further.
- **Fix options**: (a) Neumaier compensated loop for scalar, (b) multiple accumulator
  vectors for SIMD (e.g., 2-4 vectors before combining), (c) pairwise summation.

### weighted_mean_f32 Uses Naive Scalar Loop, Not SIMD
- File: `sum/mod.rs`
- The module provides SIMD-accelerated `sum_f32` but `weighted_mean_f32` uses a naive
  scalar loop. For large arrays this misses the SIMD speedup.

## Algorithm Comparison with Industry Standards

### Sigma Clipping
- **Standard approach** (Astropy, Siril, GNU Astronomy Utilities): Iteratively compute
  median and sigma (via MAD or std-dev), reject values beyond kappa*sigma, repeat.
- **This implementation**: Matches the standard approach. Uses MAD-based sigma (more robust
  than std-dev). Uses approximate median for intermediate iterations (good optimization).
- **Gap**: No convergence check on relative change (only checks if no values were clipped).
  Astropy supports `cenfunc`/`stdfunc` customization and masked arrays.

### MAD Computation
- **Standard**: MAD = median(|x_i - median(x)|), scaled by k=1.4826 for normal consistency.
- **This implementation**: Correct. Uses `MAD_TO_SIGMA = 1.4826022` which matches
  1/Phi^{-1}(3/4) to f32 precision. The computation is standard.

### SIMD Summation
- **This implementation**: Single accumulator vector per architecture. Correct results for
  typical astronomy data (values 0-65535, tile sizes up to 4096).
- **Best practice**: Use 2-4 accumulator vectors to break dependency chains and improve
  throughput. The dependency chain in single-accumulator SIMD limits ILP.
- **Correctness**: All three SIMD implementations (SSE, AVX2, NEON) are structurally
  correct. Horizontal reduction patterns are standard. Remainder handling is correct.

### Quickselect / Median
- **This implementation**: Uses Rust stdlib `select_nth_unstable_by` which is O(n) expected,
  O(n^2) worst-case (known Rust issue #102451, documented as "linear on average").
- **Even-length median**: Correctly averages two middle elements by finding the max of the
  left partition after quickselect. This is O(n) total -- standard approach.
- **Approximate median**: Uses upper-middle element only. Documented bias is negligible
  for large arrays. Good optimization for intermediate iterations.

## File Structure
```
math/
  mod.rs          - Re-exports, FWHM-sigma conversion
  statistics/
    mod.rs        - median, MAD, sigma-clipped statistics
    tests.rs      - 37 tests covering statistics, NaN handling, regression tests
    bench.rs      - Benchmarks for median and sigma clipping
  sum/
    mod.rs        - SIMD dispatch, mean, weighted_mean
    scalar.rs     - Scalar sum
    sse.rs        - SSE4.1 sum (x86_64)
    avx2.rs       - AVX2 sum (x86_64)
    neon.rs       - NEON sum (aarch64)
    tests.rs      - 19 tests including SIMD boundary and weighted_mean tests
    bench.rs      - Scalar vs SIMD benchmarks
    README.md     - Documentation
  dmat3.rs        - 3x3 f64 matrix with full test suite
  bbox.rs         - Axis-aligned bounding box with is_empty() and tests
  vec2us.rs       - 2D usize vector with tests
```

## References
- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
- [Astropy robust estimators](https://docs.astropy.org/en/stable/stats/robust.html)
- [Siril rejection algorithms](https://siril.readthedocs.io/en/stable/preprocessing/stacking.html)
- [GNU Astronomy Utilities sigma clipping](https://www.gnu.org/software/gnuastro/manual/html_node/Sigma-clipping.html)
- [MAD - Wikipedia](https://en.wikipedia.org/wiki/Median_absolute_deviation)
- [Kahan summation - Wikipedia](https://en.wikipedia.org/wiki/Kahan_summation_algorithm)
- [Pairwise summation - Wikipedia](https://en.wikipedia.org/wiki/Pairwise_summation)
- [Fast, accurate summation (blog)](http://blog.zachbjornson.com/2019/08/11/fast-float-summation.html)
- [Rust select_nth_unstable O(n^2) issue](https://github.com/rust-lang/rust/issues/102451)
- [NIST GESD test](https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm)
