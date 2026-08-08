//! Lane reductions shared by the Gaussian and Moffat SIMD backends.
//!
//! Both profile fits accumulate their normal equations in vector lanes and reduce the whole set
//! once per batch — 28 accumulators for Gaussian2D, 21 for MoffatFixedBeta — so these run per fit
//! rather than per pixel.
//!
//! The reduction is all that is shared. Each of the four backends declares, updates and reduces
//! its accumulators as flat named locals, and that repetition is deliberate: collapsing them to
//! `[__m256d; N]` / `[float64x2_t; N]` arrays driven by `for i in 0..N` was tried and measured
//! ~2-3% slower on `bench_gaussian_fit_large` (min 23.59µs against 22.82µs, six to seven runs
//! each), presumably because indexed arrays of vectors spill where named locals stay in
//! registers. The flat form is buying throughput on the fit hot path, so leave it flat.

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::{float64x2_t, vaddvq_f64};
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{__m256d, _mm256_storeu_pd};

/// Horizontal sum of 4 f64 lanes.
///
/// Declares only `avx2`, not the callers' `avx2,fma`: the body needs the wider store and nothing
/// more, and a narrower feature set still inlines into an FMA-enabled caller.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
pub(super) unsafe fn hsum(v: __m256d) -> f64 {
    let mut arr = [0.0f64; 4];
    _mm256_storeu_pd(arr.as_mut_ptr(), v);
    arr[0] + arr[1] + arr[2] + arr[3]
}

/// Horizontal sum of 2 f64 lanes.
///
/// # Safety
/// Caller must ensure running on aarch64.
#[cfg(target_arch = "aarch64")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
pub(super) unsafe fn hsum(v: float64x2_t) -> f64 {
    vaddvq_f64(v)
}
