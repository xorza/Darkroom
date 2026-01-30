//! CPU feature detection for runtime SIMD dispatch.
//!
//! This module provides cached CPU feature detection that is performed once
//! at startup. Use these functions instead of `is_x86_feature_detected!` macro
//! directly to avoid repeated CPUID calls.

use std::sync::OnceLock;

/// CPU feature flags detected once at startup.
#[derive(Debug, Clone, Copy)]
pub struct X86Features {
    pub sse2: bool,
    pub sse3: bool,
    pub sse4_1: bool,
    pub avx2: bool,
    pub fma: bool,
}

static FEATURES: OnceLock<X86Features> = OnceLock::new();

/// Get cached CPU features (detected once on first call).
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn get() -> X86Features {
    *FEATURES.get_or_init(|| X86Features {
        sse2: is_x86_feature_detected!("sse2"),
        sse3: is_x86_feature_detected!("sse3"),
        sse4_1: is_x86_feature_detected!("sse4.1"),
        avx2: is_x86_feature_detected!("avx2"),
        fma: is_x86_feature_detected!("fma"),
    })
}

/// Get cached CPU features - stub for non-x86 platforms.
#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn get() -> X86Features {
    X86Features {
        sse2: false,
        sse3: false,
        sse4_1: false,
        avx2: false,
        fma: false,
    }
}

/// Check if SSE2 is available.
#[inline]
pub fn has_sse2() -> bool {
    get().sse2
}

/// Check if SSE3 is available.
#[inline]
pub fn has_sse3() -> bool {
    get().sse3
}

/// Check if SSE4.1 is available.
#[inline]
pub fn has_sse4_1() -> bool {
    get().sse4_1
}

/// Check if AVX2 is available.
#[inline]
pub fn has_avx2() -> bool {
    get().avx2
}

/// Check if AVX2 and FMA are both available (common requirement for fast math).
#[inline]
pub fn has_avx2_fma() -> bool {
    let f = get();
    f.avx2 && f.fma
}
