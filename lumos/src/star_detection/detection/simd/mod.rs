//! SIMD-accelerated detection algorithms.

#[cfg(target_arch = "aarch64")]
pub mod neon;
#[cfg(target_arch = "x86_64")]
pub mod sse;
