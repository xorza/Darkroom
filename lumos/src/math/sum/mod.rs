//! Sum and accumulation operations with SIMD acceleration.
//!
//! Every backend accumulates in f64 and nothing rounds until a caller asks for an f32, so the
//! rounding happens once and at the edge. [`sum_f32`] is the primitive the others are built from;
//! [`mean_f32`] and [`weighted_mean_f32`] are the two routes the combine takes to a pixel, and they
//! are required to agree on it.
//!
//! Backend selection is gated on each vector's structural minimum — below one full vector there is
//! nothing to widen — everywhere except AVX2 [`sum_f32`], whose fallback is itself vectorized and so
//! takes a measured crossover to beat. `bench.rs` is what says which of the two shapes a gate wants,
//! and `.notes/simd-todo.md` carries the numbers.

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

/// Length at which AVX2 [`sum_f32`] overtakes its fallback — measured, not structural.
///
/// Every other gate here is the lane minimum, because below one full vector there is nothing to
/// widen. This one cannot be, because on x86_64 the fallback is not a scalar loop: SSE2 is baseline,
/// so LLVM auto-vectorizes [`scalar::sum_f32`] into a 4-wide f64 accumulation and the AVX2 kernel
/// has to beat *that*. One vector's worth of work does not amortize the reduction — at the 8-lane
/// minimum the kernel runs 0.80x its fallback, breaks even at 10, and only pulls clear at 16
/// (1.71x). [`weighted_sums()`] has no such gap and keeps the lane minimum: its fallback carries two
/// accumulators and a multiply, which LLVM vectorizes less well, so it is ahead from 8 elements up.
///
/// Set from `bench_sum_f32_crossover`; `.notes/simd-todo.md` carries the sweep it came from.
#[cfg(target_arch = "x86_64")]
const AVX2_SUM_F32_CROSSOVER: usize = 16;

/// Sum f32 values, returning the unrounded f64 total.
///
/// The suffix names the element type, not the return type: this takes f32 samples and hands back
/// the wider accumulator it built them in, rather than rounding on the way out. A caller that
/// splits its input — `par_chunks` over an image plane, say — must be able to combine the partial
/// sums without dropping to f32 between them, or the wide accumulator buys nothing at exactly the
/// length it matters most.
pub(crate) fn sum_f32(values: &[f32]) -> f64 {
    dispatch! {
        x86: avx2 if values.len() >= AVX2_SUM_F32_CROSSOVER => avx2::sum_f32(values),
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
/// Agrees with [`mean_f32`] bit for bit when the weights are equal *and both reach the same rung*:
/// `v * w` is exact in f64, and each backend accumulates the numerator with the same lane split,
/// reduction order and scalar tail as its own [`sum_f32`], so unit weights walk the identical values
/// through the identical additions.
///
/// The two do not reach the same rung everywhere. On x86 this gates at [`AVX2_F32_LANES`] while
/// [`sum_f32`] waits for [`AVX2_SUM_F32_CROSSOVER`], so from 8 to 15 elements the weighted numerator
/// reassociates into lanes while the plain mean is still accumulating sequentially. On values that
/// cancel, that window puts the two up to ~500 f32 ULPs apart. Nothing in the pipeline compares them
/// there — the combine only ever arrives through this function, and [`mean_f32`]'s one caller is
/// sigma-clipped statistics — so the window is documented rather than closed. Closing it would mean
/// giving up the vector arm at exactly the frame counts a stack is most often built from.
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
