//! Vector backends for the compensated sums, and the dispatch between them.

use crate::math::sum::scalar;
#[cfg(target_arch = "aarch64")]
use crate::simd::NEON_F32_LANES;
use crate::simd::dispatch;

#[cfg(all(test, feature = "internals"))]
mod bench;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse41;

/// Throughput crossovers, not structural minimums: both sit far above the 8-lane AVX2 and 4-lane
/// SSE4.1 vector widths, so below them the scalar loop wins on a length the kernels could handle.
/// Set from `bench_sum_f32_crossover` and `bench_weighted_mean_f32_crossover` (`bench.rs`), which
/// sweep each backend against scalar over `CROSSOVER_SIZES`.
///
/// `X86_WEIGHTED_MEAN_CROSSOVER` gates the AVX2 and the SSE4.1 rung on one number even though they
/// are 8- and 4-lane kernels, which looks like an 8-lane measurement borrowed by a 4-lane path.
/// Measured, it is right for both: at n=64 both lose to scalar (7.79µs and 7.65µs against 6.79µs),
/// at n=128 both win (5.12µs and 6.22µs against 6.91µs). Above it AVX2 runs ~2x scalar and SSE4.1
/// ~1.35x, so the SSE rung is worth having on the pre-AVX2 machines that are the only ones to
/// reach it.
#[cfg(target_arch = "x86_64")]
const AVX2_SUM_CROSSOVER: usize = 256;
#[cfg(target_arch = "x86_64")]
const X86_WEIGHTED_MEAN_CROSSOVER: usize = 128;

/// Sum f32 values using SIMD when available.
///
/// The NEON arm has no crossover of its own — one full vector is enough for it to win — so it is
/// gated on the structural minimum instead.
///
/// There is deliberately no SSE4.1 rung here, unlike [`weighted_mean_f32`]. It would only ever run
/// on pre-AVX2 x86_64, where the fallback is already LLVM's SSE2 auto-vectorization of
/// `scalar::sum_f32` rather than a true scalar loop — the same situation that leaves the weighted
/// mean's hand-written SSE kernel only ~1.35x ahead. A second unsafe kernel and its cross-checks,
/// carried forever, to win about that much on hardware from 2013 and earlier is not worth it.
pub(crate) fn sum_f32(values: &[f32]) -> f32 {
    dispatch! {
        x86: avx2 if values.len() >= AVX2_SUM_CROSSOVER => avx2::sum_f32(values),
        aarch64 if values.len() >= NEON_F32_LANES => neon::sum_f32(values),
        scalar => scalar::sum_f32(values),
    }
}

/// Compute weighted mean of values with corresponding weights using SIMD when available.
///
/// Uses Kahan compensated summation for SIMD and wider scalar accumulation.
/// Returns 0.0 if the total weight is near zero.
pub(crate) fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    // Release assert, not debug: the SIMD backends walk `weights` through a raw pointer, so a
    // shorter `weights` is an out-of-bounds read (UB) in release, not a recoverable error.
    assert_eq!(
        values.len(),
        weights.len(),
        "values and weights must have the same length"
    );

    if values.is_empty() {
        return 0.0;
    }

    dispatch! {
        x86: avx2 if values.len() >= X86_WEIGHTED_MEAN_CROSSOVER
            => avx2::weighted_mean_f32(values, weights),
        x86: sse4_1 if values.len() >= X86_WEIGHTED_MEAN_CROSSOVER
            => sse41::weighted_mean_f32(values, weights),
        aarch64 if values.len() >= NEON_F32_LANES => neon::weighted_mean_f32(values, weights),
        scalar => scalar::weighted_mean_f32(values, weights),
    }
}
