//! SIMD implementations for cosmic ray detection.
//!
//! Currently a placeholder - SIMD optimization not yet implemented.

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;
