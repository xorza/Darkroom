//! Lane reductions shared by the Gaussian and Moffat SIMD backends.
//!
//! Both profile fits accumulate their normal equations in vector lanes and reduce the whole set
//! once per batch — 28 accumulators for Gaussian2D, 21 for MoffatFixedBeta — so these run per fit
//! rather than per pixel.

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
