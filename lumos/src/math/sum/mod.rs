//! Sums and means of f32 samples.
//!
//! Every path accumulates in f64 and nothing rounds until a caller asks for an f32, so the
//! rounding happens once and at the edge. [`sum_f32`] is the primitive the others are built from;
//! [`mean_f32`] and [`weighted_mean_f32`] are the two routes the combine takes to a pixel, and they
//! are required to agree on it.
//!
//! Both take the vector kernel from one vector up, and the sequential scalar loop below it: one
//! gate, so at every length the two walk the same values through the same additions.

mod scalar;
mod simd;
mod weighted_sums;

use crate::math::sum::simd::{SumF32, WeightedSumsKernel};
use crate::math::sum::weighted_sums::WeightedSums;
use crate::simd::{F32_LANES, Kernel};

/// Sum f32 values, returning the unrounded f64 total.
///
/// The suffix names the element type, not the return type: this takes f32 samples and hands back
/// the wider accumulator it built them in, rather than rounding on the way out. A caller that
/// splits its input — `par_chunks` over an image plane, say — must be able to combine the partial
/// sums without dropping to f32 between them, or the wide accumulator buys nothing at exactly the
/// length it matters most.
fn sum_f32(values: &[f32]) -> f64 {
    if values.len() >= F32_LANES {
        SumF32(values).dispatch()
    } else {
        scalar::sum_f32(values)
    }
}

/// Mean of f32 values, rounded to f32 exactly once.
///
/// An empty slice is a logic error rather than a zero: a mean of nothing has no value to return,
/// and every caller either has frames or has already checked that it does.
pub(crate) fn mean_f32(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty(), "mean of an empty slice");
    (sum_f32(values) / values.len() as f64) as f32
}

/// Weighted mean of f32 values, rounded to f32 exactly once.
///
/// Agrees with [`mean_f32`] bit for bit when the weights are equal: `v * w` is exact in f64, both
/// take the vector kernel from the same length, and the weighted kernel accumulates its numerator
/// with the same lane split, fold and padded last vector as the plain one, so unit weights walk
/// the identical values through the identical additions.
///
/// A zero total weight returns 0.0 — every frame contributing to this pixel was rejected or
/// distrusted, which is data, not a fault. An empty slice is a logic error, as it is for
/// [`mean_f32`].
pub(crate) fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    debug_assert!(!values.is_empty(), "weighted mean of an empty slice");
    debug_assert_eq!(
        values.len(),
        weights.len(),
        "values and weights must have the same length"
    );

    let sums = weighted_sums(values, weights);
    // On the total rather than per weight: this runs once per output pixel, so an O(n) scan of the
    // weights would cost more in debug than the combine itself. A negative total is the only way
    // negative weights reach an answer, and it would otherwise be indistinguishable from the
    // legitimate all-zero case below.
    debug_assert!(
        sums.weight_total >= 0.0,
        "weights are frame trust factors and cannot sum negative"
    );
    if sums.weight_total > 0.0 {
        (sums.weighted_values / sums.weight_total) as f32
    } else {
        0.0
    }
}

/// Both weighted-mean totals, from the vector kernel at one vector and up, as [`sum_f32`].
///
/// The division stays with the caller so the near-zero-weight decision is stated once.
fn weighted_sums(values: &[f32], weights: &[f32]) -> WeightedSums {
    if values.len() >= F32_LANES {
        WeightedSumsKernel { values, weights }.dispatch()
    } else {
        scalar::weighted_sums(values, weights)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
