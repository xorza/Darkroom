//! Sum and accumulation operations with SIMD acceleration.

use crate::simd::dispatch;

pub(crate) mod scalar;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "x86_64")]
mod sse;

/// One full NEON vector of f32 — the shortest input the kernel can do any vector work on.
#[cfg(target_arch = "aarch64")]
const NEON_MIN_LEN: usize = 4;

/// Throughput crossovers, not structural minimums: both sit far above the 8-lane AVX2 and 4-lane
/// SSE4.1 vector widths, so below them the scalar loop wins on a length the kernels could handle.
#[cfg(target_arch = "x86_64")]
const AVX2_SUM_MIN_LEN: usize = 256;
#[cfg(target_arch = "x86_64")]
const X86_WEIGHTED_MEAN_MIN_LEN: usize = 128;

/// Sum f32 values using SIMD when available.
pub(crate) fn sum_f32(values: &[f32]) -> f32 {
    dispatch! {
        x86: avx2 if values.len() >= AVX2_SUM_MIN_LEN => avx2::sum_f32(values),
        aarch64 if values.len() >= NEON_MIN_LEN => neon::sum_f32(values),
        scalar => scalar::sum_f32(values),
    }
}

/// Mean of f32 values, rounded to f32 exactly once.
///
/// Accumulates and divides in f64 rather than reusing the SIMD [`sum_f32`]. Two reasons, and
/// both are about the result rather than the loop:
///
/// - Rounding the sum to f32 and *then* dividing rounds twice, landing up to an extra ULP from
///   the exact mean.
/// - This is the unit-weight case of [`weighted_mean_f32`], and the combine reaches the same
///   pixel through either function depending on whether frame weights are in play. Sharing the
///   f64 accumulate-then-divide shape makes the two agree bit-for-bit below the weighted mean's
///   SIMD threshold, so which entry point a stack takes cannot change its output.
///
/// Every caller averages over a frame count (tens), far below where the SIMD sum would pay.
pub(crate) fn mean_f32(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());
    (scalar::sum_f64(values) / values.len() as f64) as f32
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
        x86: avx2 if values.len() >= X86_WEIGHTED_MEAN_MIN_LEN
            => avx2::weighted_mean_f32(values, weights),
        x86: sse4_1 if values.len() >= X86_WEIGHTED_MEAN_MIN_LEN
            => sse::weighted_mean_f32(values, weights),
        aarch64 if values.len() >= NEON_MIN_LEN => neon::weighted_mean_f32(values, weights),
        scalar => scalar::weighted_mean_f32(values, weights),
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "internals"))]
mod bench;
