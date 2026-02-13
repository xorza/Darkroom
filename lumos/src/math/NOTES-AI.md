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
- Correct DMat3: cofactor inverse, perspective divide, Frobenius norm
- Good test coverage for statistics and SIMD boundaries

## Issues Found

### BUG: Deviation/Value Index Mismatch in sigma_clip_iteration
- File: `statistics/mod.rs`, lines 96-139
- **Root cause**: Two calls to `select_nth_unstable_by` destroy index correspondence.
  1. Line 109: `median_f32_approx(active)` partially sorts `values[..*len]`.
  2. Lines 112-113: `deviations` is copied from the reordered values, then transformed to
     absolute deviations. At this point `deviations[i]` corresponds to `values[i]`.
  3. Line 115: `median_f32_approx(&mut deviations[..*len])` partially sorts `deviations`.
     Now `deviations[i]` no longer corresponds to `values[i]`.
  4. Lines 125-129: Clipping loop uses `deviations[i]` to decide on `values[i]` -- indices are mismatched.
- **Effect**: Wrong values get clipped. The pairing is effectively randomized.
- **Mitigation in practice**: Deviations are symmetric around the median, and multiple
  iterations re-converge, so final results are often close. But individual iterations
  clip incorrect values, and edge cases (asymmetric distributions) will produce wrong results.
- **Fix**: After computing the MAD median, recompute deviations from scratch:
  ```rust
  // After line 115 (MAD computation), recompute:
  for i in 0..*len {
      deviations[i] = (values[i] - median).abs();
  }
  ```
  Alternatively, use a separate scratch buffer for the MAD median computation.

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

### NaN Input Panics via partial_cmp().unwrap()
- File: `statistics/mod.rs`, lines 36, 40, 59
- `select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap())` panics if any value is NaN.
- NaN values can arise from dead/hot pixels, division by zero in calibration, or bad FITS data.
- **Industry standard**: Astropy's `sigma_clip` has `masked=True` to handle NaN/Inf.
  Rust's `f32::total_cmp` provides a total order (NaN sorts to end) with no performance penalty.
- **Fix**: Replace `|a, b| a.partial_cmp(b).unwrap()` with `f32::total_cmp`. NaN values
  will sort to the end and be naturally clipped by sigma clipping.

### weighted_mean_f32 Has No Tests
- File: `sum/mod.rs`, lines 41-59
- Zero test coverage; violates project CLAUDE.md requirement to test all non-GUI code.
- Needs tests for: normal case, mismatched lengths, zero weights, negative weights, empty input.

### weighted_mean_f32 Silently Truncates Mismatched Lengths
- File: `sum/mod.rs`, line 49
- `zip()` silently truncates to the shorter slice.
- **Fix**: Add `debug_assert_eq!(values.len(), weights.len())` to catch caller bugs.

### weighted_mean_f32 Fallback on Zero Weights Masks Bugs
- File: `sum/mod.rs`, lines 54-58
- When `weight_sum <= f32::EPSILON`, falls back to unweighted mean.
- This masks likely caller bugs (all-zero or all-negative weights).
- **Fix**: Return 0.0 or document the behavior explicitly. Falling back to unweighted mean
  is surprising behavior.

### weighted_mean_f32 Uses Naive Scalar Loop, Not SIMD
- File: `sum/mod.rs`, lines 46-51
- The module provides SIMD-accelerated `sum_f32` but `weighted_mean_f32` uses a naive
  scalar loop. For large arrays this misses the SIMD speedup.

### transform_point Division by Zero When w=0
- File: `dmat3.rs`, lines 129-135
- Projective transforms can have w=0 (points at infinity), producing Inf/NaN.
- **Fix**: Add `debug_assert!(w.abs() > f64::EPSILON)` to catch this in debug builds.

### DMat3::inverse Singularity Threshold Not Scale-Invariant
- File: `dmat3.rs`, line 100
- Hardcoded threshold `1e-12` is not appropriate for all coordinate scales.
- OK for pixel-scale coordinates (typical values 0-10000) but should document this assumption.

### Stale sum/README.md
- File: `sum/README.md`
- References removed functions: `sum_squared_diff`, `accumulate`, `scale`.
- Table lists 5 functions but only `sum_f32`, `mean_f32`, and `weighted_mean_f32` exist.
- Benchmark results may be outdated.

### Aabb::empty().width() Returns 1, Not 0
- File: `bbox.rs`, lines 47-49
- `saturating_sub` prevents overflow but `usize::MAX.saturating_sub(0) + 1 = 0` (wraps).
  Actually on closer inspection: `max=0, min=usize::MAX`, so `width = 0 - usize::MAX`
  which saturates to 0, then +1 = 1. This is semantically wrong for an empty box.
- **Fix**: Add `is_empty()` method; return 0 from `width()`/`height()`/`area()` when empty.

### Vec2us::sub Can Panic on Underflow
- File: `vec2us.rs`, lines 53-58
- Unsigned subtraction panics in debug mode on underflow. `Aabb` uses `saturating_sub`
  internally but `Vec2us` does not.

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
    tests.rs      - 30 tests covering statistics functions
    bench.rs      - Benchmarks for median and sigma clipping
  sum/
    mod.rs        - SIMD dispatch, mean, weighted_mean
    scalar.rs     - Scalar sum
    sse.rs        - SSE4.1 sum (x86_64)
    avx2.rs       - AVX2 sum (x86_64)
    neon.rs       - NEON sum (aarch64)
    tests.rs      - 12 tests including SIMD boundary tests
    bench.rs      - Scalar vs SIMD benchmarks
    README.md     - (stale) documentation
  dmat3.rs        - 3x3 f64 matrix with full test suite
  bbox.rs         - Axis-aligned bounding box with tests
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
