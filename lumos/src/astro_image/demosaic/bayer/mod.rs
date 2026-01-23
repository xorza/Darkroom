//! Bayer CFA demosaicing implementations (scalar and SIMD).
//!
//! This module contains the bilinear interpolation implementations for Bayer pattern
//! demosaicing. The module provides:
//! - A scalar (non-SIMD) implementation that works on all platforms
//! - SSE3 SIMD implementation for x86_64
//! - NEON SIMD implementation for aarch64

pub(crate) mod scalar;

#[cfg(target_arch = "x86_64")]
pub(crate) mod simd_sse3;

#[cfg(target_arch = "aarch64")]
pub(crate) mod simd_neon;

#[cfg(feature = "bench")]
pub mod bench;
