//! Sum and accumulation operations with SIMD acceleration.
//!
//! Every backend accumulates in f64 and nothing rounds until a caller asks for an f32, so the
//! rounding happens once and at the edge. [`sum_f32`] is the primitive the others are built from;
//! [`mean_f32`] and [`weighted_mean_f32`] are the two routes the combine takes to a pixel, and they
//! are required to agree on it.
//!
//! Backend selection is gated on each vector's structural minimum — below one full vector there is
//! nothing to widen — rather than on a measured throughput crossover. `bench.rs` is what says that
//! is right, and `.notes/simd-todo.md` carries the numbers and what is still unmeasured.

#[cfg(target_arch = "x86_64")]
mod avx2;
#[cfg(target_arch = "aarch64")]
mod neon;
mod scalar;
mod weighted_sums;

#[cfg(all(test, feature = "internals"))]
mod bench;

use crate::math::sum::weighted_sums::WeightedSums;
#[cfg(target_arch = "x86_64")]
use crate::simd::AVX2_F32_LANES;
#[cfg(target_arch = "aarch64")]
use crate::simd::NEON_F32_LANES;
use crate::simd::dispatch;

/// Sum f32 values, returning the unrounded f64 total.
///
/// The suffix names the element type, not the return type: this takes f32 samples and hands back
/// the wider accumulator it built them in, rather than rounding on the way out. A caller that
/// splits its input — `par_chunks` over an image plane, say — must be able to combine the partial
/// sums without dropping to f32 between them, or the wide accumulator buys nothing at exactly the
/// length it matters most.
pub(crate) fn sum_f32(values: &[f32]) -> f64 {
    dispatch! {
        x86: avx2 if values.len() >= AVX2_F32_LANES => avx2::sum_f32(values),
        aarch64 if values.len() >= NEON_F32_LANES => neon::sum_f32(values),
        scalar => scalar::sum_f32(values),
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
/// Agrees with [`mean_f32`] bit for bit when the weights are equal, by construction rather than by
/// luck: `v * w` is exact in f64, and every backend accumulates the numerator with the same lane
/// split, reduction order and scalar tail as its [`sum_f32`], so unit weights walk the identical
/// values through the identical additions. The combine reaches the same pixel through either
/// function depending on whether frame weights are in play, so a disagreement between them would
/// make a stack's output depend on which entry point it took.
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

/// Both weighted-mean totals, from whichever backend fits.
///
/// The division stays with the caller so the near-zero-weight decision is stated once instead of
/// once per architecture.
fn weighted_sums(values: &[f32], weights: &[f32]) -> WeightedSums {
    dispatch! {
        x86: avx2 if values.len() >= AVX2_F32_LANES => avx2::weighted_sums(values, weights),
        aarch64 if values.len() >= NEON_F32_LANES => neon::weighted_sums(values, weights),
        scalar => scalar::weighted_sums(values, weights),
    }
}

#[cfg(test)]
mod tests;
