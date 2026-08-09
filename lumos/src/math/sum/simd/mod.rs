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
#[cfg(target_arch = "x86_64")]
const AVX2_SUM_CROSSOVER: usize = 256;
#[cfg(target_arch = "x86_64")]
const X86_WEIGHTED_MEAN_CROSSOVER: usize = 128;

/// Sum f32 values using SIMD when available.
///
/// The NEON arm has no crossover of its own — one full vector is enough for it to win — so it is
/// gated on the structural minimum instead.
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
