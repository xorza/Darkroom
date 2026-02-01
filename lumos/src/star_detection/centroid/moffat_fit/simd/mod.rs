//! SIMD-accelerated Moffat profile computations.
//!
//! Uses AVX2 to process 8 pixels in parallel for Jacobian and residual computation.

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(test)]
mod tests;

#[cfg(target_arch = "x86_64")]
pub(crate) use avx2::*;

/// Check if AVX2 is available at runtime.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn is_avx2_available() -> bool {
    is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma")
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn is_avx2_available() -> bool {
    false
}
