//! Sums and means of f32 samples.
//!
//! Every path accumulates in f64 and nothing rounds until a caller asks for an f32, so the
//! rounding happens once and at the edge. [`sum_f32`] is the primitive the others are built from;
//! [`mean_f32`] and [`weighted_mean_f32`] are the two routes the combine takes to a pixel, and they
//! are required to agree on it.
//!
//! Each entry point takes the vector kernel from a length up, and the sequential scalar loop below
//! it. The gate is a measured crossover, not a structural minimum: the kernel handles any length,
//! its partial last vector zero-padded, but below the gate the scalar loop is faster.

mod scalar;
mod simd;
mod weighted_sums;

#[cfg(all(test, feature = "bench"))]
mod bench;

use rayon::prelude::*;

use crate::math::sum::simd::{SumF32, WeightedSumsKernel};
use crate::math::sum::weighted_sums::WeightedSums;
use crate::simd::{F32_LANES, Kernel};

/// Length at which the vector [`sum_f32`] overtakes the scalar loop — measured, not structural.
///
/// On `x86_64` the scalar loop is not scalar: SSE2 is baseline, so LLVM auto-vectorizes
/// [`scalar::sum_f32`] into a 4-wide f64 accumulation, and the AVX2 kernel has to beat *that*.
/// One vector's worth of work does not amortize the fold — at 8 elements the kernel ran 0.80x its
/// fallback, broke even at 10, and only pulled clear at 16 (1.71x). [`weighted_sums()`] has no
/// such gap and gates at one vector: its scalar loop carries two accumulators and a multiply, which
/// LLVM vectorizes less well. Set on AVX2 from `bench_sum_f32_crossover`, and held on every Isa
/// so the two entry points' window below stays one window on every CPU.
const SUM_F32_CROSSOVER: usize = 16;

/// Sum f32 values, returning the unrounded f64 total.
///
/// The suffix names the element type, not the return type: this takes f32 samples and hands back
/// the wider accumulator it built them in, rather than rounding on the way out. A caller that
/// splits its input — `par_chunks` over an image plane, say — must be able to combine the partial
/// sums without dropping to f32 between them, or the wide accumulator buys nothing at exactly the
/// length it matters most.
pub(crate) fn sum_f32(values: &[f32]) -> f64 {
    if values.len() >= SUM_F32_CROSSOVER {
        SumF32(values).dispatch()
    } else {
        scalar::sum_f32(values)
    }
}

/// Samples one task of [`par_sum_f32`] sums.
const PAR_SUM_CHUNK: usize = 1 << 16;

/// [`sum_f32`] across threads, the same bits on any thread count: fixed chunks of
/// [`PAR_SUM_CHUNK`] samples, each summed by [`sum_f32`], and their partial sums added in order.
/// rayon's own `sum` splits where its work stealing splits, so its total moves with the scheduling.
pub(crate) fn par_sum_f32(values: &[f32]) -> f64 {
    values
        .par_chunks(PAR_SUM_CHUNK)
        .map(sum_f32)
        .collect::<Vec<f64>>()
        .iter()
        .sum()
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
/// Agrees with [`mean_f32`] bit for bit when the weights are equal *and both take the same
/// path*: `v * w` is exact in f64, and the weighted kernel accumulates its numerator with the same
/// lane split, fold and padded last vector as the plain one, so unit weights walk the identical
/// values through the identical additions.
///
/// The two do not take the same path everywhere. This gates at [`F32_LANES`] while [`sum_f32`]
/// waits for [`SUM_F32_CROSSOVER`], so from 8 to 15 elements the weighted numerator reassociates
/// into lanes while the plain mean is still accumulating sequentially. On values that cancel, that
/// window puts the two up to ~500 f32 ULPs apart. Nothing in the pipeline compares them there —
/// the combine only ever arrives through this function, and [`mean_f32`]'s one caller is
/// sigma-clipped statistics — so the window is documented rather than closed. Closing it would
/// mean giving up the vector path at exactly the frame counts a stack is most often built from.
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

/// Both weighted-mean totals, from the vector kernel at one vector and up.
///
/// The division stays with the caller so the near-zero-weight decision is stated once.
fn weighted_sums(values: &[f32], weights: &[f32]) -> WeightedSums {
    if values.len() >= F32_LANES {
        WeightedSumsKernel { values, weights }.dispatch()
    } else {
        scalar::weighted_sums(values, weights)
    }
}

#[cfg(test)]
mod tests;
