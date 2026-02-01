//! SIMD-accelerated Moffat profile computations.
//!
//! Uses AVX2 to process 8 pixels in parallel for Jacobian and residual computation.

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(test)]
mod tests;

#[cfg(target_arch = "x86_64")]
pub(crate) use avx2::*;

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

/// Check if AVX2+FMA is available at runtime.
#[inline]
pub fn is_avx2_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        cpu_features::has_avx2_fma()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}
