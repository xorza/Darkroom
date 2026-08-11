//! Scalar (non-SIMD) implementations of sum operations.
//!
//! These are both the fallback arm of the dispatch and the tail of every vector kernel, which is
//! why they take arbitrary-length slices rather than assuming a remainder shorter than one vector.

use crate::math::sum::weighted_sums::WeightedSums;

/// Sum f32 values into an f64 accumulator, unrounded.
///
/// Naive f64 accumulation beats f32 compensated summation by orders of magnitude at any length
/// this crate sums (error `n·2⁻⁵³` against `2·2⁻²⁴`), so the wider accumulator *is* the accurate
/// choice, not a fallback for one.
#[inline]
pub(super) fn sum_f32(values: &[f32]) -> f64 {
    values.iter().map(|&value| f64::from(value)).sum::<f64>()
}

/// Both weighted-mean totals over the same elements.
///
/// Each `f32 → f64` widening is lossless and the f64 product of two f32s is exact (24+24 bits fit
/// in 53), so every term entering these sums is exact and only the additions carry error.
///
/// Iterating with `zip` stops at the shorter slice; callers assert the lengths match rather than
/// relying on that, because a silent truncation is a wrong answer rather than a safe one.
#[inline]
pub(super) fn weighted_sums(values: &[f32], weights: &[f32]) -> WeightedSums {
    let mut weighted_values = 0.0f64;
    let mut weight_total = 0.0f64;

    for (&value, &weight) in values.iter().zip(weights) {
        weighted_values += f64::from(value) * f64::from(weight);
        weight_total += f64::from(weight);
    }

    WeightedSums {
        weighted_values,
        weight_total,
    }
}
