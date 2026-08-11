//! Vector backends for the wide-accumulator sums, and the dispatch between them.

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
///
/// Both figures predate widening the kernels to f64 accumulators, and neither is the number its
/// benchmark would pick today. They are kept because they are still *safe* gates — a crossover set
/// too high only ever routes more work to the scalar path, which is the accurate one — but both
/// want re-measuring on x86_64 hardware before they are trusted as tuned values. Expect them to
/// fall rather than rise: the same widening on aarch64 moved the weighted mean's crossover down to
/// the structural minimum, because dropping the f32 compensation frees four dependent ops per
/// accumulate and more than pays for retiring half as many lanes.
#[cfg(target_arch = "x86_64")]
const AVX2_SUM_CROSSOVER: usize = 256;
#[cfg(target_arch = "x86_64")]
const X86_WEIGHTED_MEAN_CROSSOVER: usize = 128;

/// Sum f32 values using SIMD when available.
///
/// Every backend accumulates in f64 and rounds once at the end, so all three agree with
/// [`scalar::sum_f32`] on the f64 sum rounded once. Compensating in f32 only approached that:
/// Neumaier bounds the *summation* error at ~2·2⁻²⁴ regardless of length, which is still an f32
/// ULP, where a naive f64 accumulator carries n·2⁻⁵³ and stays below f32's granularity until
/// n ≈ 2²⁹ — past anything this crate sums.
///
/// The wider accumulator is also the faster one, which is the opposite of what halving the lane
/// count suggests. Compensation costs four *dependent* ops per accumulate through a single serial
/// chain, so the old kernels were latency-bound; f64 costs one add into two independent
/// accumulators. Measured by `bench_sum_f32` on aarch64-apple-darwin at 10k elements, the f64
/// kernel takes 1.46µs against the f32-Kahan kernel's 4.83µs — which was itself 4.83µs against
/// scalar's 4.83µs, i.e. the compensated NEON rung was worth nothing at all over the fallback.
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
/// Every backend accumulates the numerator and the denominator in f64, matching
/// [`scalar::weighted_mean_f32`] and [`crate::math::sum::mean_f32`]. That is a correctness
/// requirement, not a precision luxury: the combine reaches the same pixel through `mean_f32` with
/// equal weights or through this function with frame weights, so a backend that accumulated in
/// compensated f32 made the stacked value depend on which entry point the caller took — and, since
/// the arms are gated at different lengths per architecture, on which architecture ran it. f32
/// Kahan carries ~2·2⁻²⁴ of error against f64's n·2⁻⁵³, enough to shift the rounded f32 result by
/// an ULP.
///
/// The lane order still differs from the scalar loop, so the f64 sums are not bit-identical to it;
/// the f64 headroom over f32 is what makes the *rounded* results agree.
///
/// Widening cost nothing on aarch64 — it paid. Dropping the f32 Kahan step removes four dependent
/// ops per accumulate and its single serial chain, which more than covers retiring half as many
/// lanes per instruction: measured by `bench_weighted_mean_f32` on aarch64-apple-darwin, the f64
/// kernel runs 10k elements in 1.71µs against the f32-Kahan kernel's 5.17µs and scalar's 5.67µs.
/// The old kernel was barely ahead of scalar at all. The NEON arm is gated on `NEON_F32_LANES`
/// rather than a measured crossover because `bench_weighted_mean_f32_crossover` puts it level with
/// scalar at n=4 and ahead from n=8 up (1.33x at 8, 1.97x at 64, 3.16x at 1024), so the structural
/// minimum *is* the crossover here.
///
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
