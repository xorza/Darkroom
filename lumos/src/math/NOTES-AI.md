# math Module - Code Review vs Industry Standards

## Overview
Numerical primitives: SIMD-accelerated summation (AVX2/SSE4.1/NEON), mean, weighted mean, robust statistics (median via quickselect, MAD, iterative sigma-clipped rejection), 3x3 f64 matrix for projective transforms, bounding box, 2D usize vector, FWHM-sigma conversions.

## What It Does Well
- MAD-based sigma clipping (industry standard for astro, matches Astropy/Siril)
- Efficient O(n) median via quickselect
- Approximate median optimization for intermediate sigma-clipping iterations
- SIMD dispatch hierarchy: AVX2 > SSE4.1 > scalar (NEON always on aarch64)
- Zero-allocation ArrayVec variant for hot per-tile loops
- Correct DMat3: cofactor inverse, perspective divide, Frobenius norm
- Standard Aabb accumulation pattern with saturating_sub
- Good test coverage

## Issues Found

### BUG: Deviation/Value Index Mismatch in sigma_clip_iteration
- File: statistics/mod.rs:108-122
- median_f32_approx(active) reorders values[..*len] (line 108)
- deviations copied from reordered values, then abs_deviation_inplace (lines 111-112)
- median_f32_approx(deviations) reorders deviations again (line 114)
- Clipping loop uses deviations[i] to decide on values[i] but indices no longer correspond
- Effect: wrong values get clipped - random pairing instead of correct pairing
- Muted in practice (deviations symmetric, multiple iterations converge) but introduces randomness
- Fix: Recompute deviations after MAD median call, or use scratch buffer for MAD

### Critical: No Compensated Summation - Precision Loss for Large Arrays
- Files: sum/scalar.rs:4, sum/sse.rs, sum/avx2.rs, sum/neon.rs
- All use naive sequential accumulation
- f32 with ~7 digits: summing N values accumulates O(N) roundoff error
- For 4096-pixel tile of values ~100.0, can lose 1-2 significant digits
- SIMD lanes provide some pairwise benefit, but horizontal reduction and remainder are naive
- Fix: Neumaier compensated summation for scalar, multiple accumulator vectors for SIMD

### High: weighted_mean_f32 Has No Tests
- File: sum/mod.rs:41-57
- Zero test coverage; violates project's CLAUDE.md rules

### High: weighted_mean_f32 Silently Truncates Mismatched Lengths
- File: sum/mod.rs:48
- zip() truncates to shorter slice without warning
- Fix: Add debug_assert_eq!(values.len(), weights.len())

### Medium: weighted_mean_f32 Fallback on Zero Weights Masks Bugs
- File: sum/mod.rs:52-55
- Falls back to unweighted mean when weights sum to zero
- Masks likely caller bug; should return NaN or 0.0

### Medium: weighted_mean_f32 Uses Naive Scalar Loop, Not SIMD
- File: sum/mod.rs:48-50
- Module exists for SIMD summation but weighted_mean is scalar

### Medium: NaN Input Panics via partial_cmp().unwrap()
- File: statistics/mod.rs:28,31,50,51
- NaN values can arise from dead pixels, division by zero in calibration
- Fix: Use f32::total_cmp instead (NaN sorts to end, no performance penalty)

### Medium: transform_point Division by Zero When w=0
- File: dmat3.rs:116-119
- Projective transforms can have w=0 (points at infinity)
- Fix: Add debug_assert!(w.abs() > f64::EPSILON)

### Medium: DMat3::inverse Singularity Threshold Not Scale-Invariant
- File: dmat3.rs:99
- Hardcoded 1e-12 not appropriate for all coordinate scales
- OK for pixel-scale coordinates but should document the assumption

### Low: Stale sum/README.md
- References removed functions: sum_squared_diff, accumulate, scale

### Low: Aabb::empty().width() Returns 1, Not 0
- File: bbox.rs:47-49
- saturating_sub prevents overflow but produces wrong answer for empty box
- Fix: Add is_empty() method, return 0 from width()/height()/area() when empty

### Low: Vec2us::sub Can Panic on Underflow
- File: vec2us.rs:46-51
- Unsigned subtraction panics in debug mode
- Aabb uses saturating_sub but Vec2us does not
