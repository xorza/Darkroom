//! CPU feature detection for runtime SIMD dispatch.
//!
//! Cached on first call; use these instead of `is_x86_feature_detected!` directly.

#[cfg(target_arch = "x86_64")]
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub struct X86Features {
    pub sse2: bool,
    pub sse3: bool,
    pub ssse3: bool,
    pub sse4_1: bool,
    pub avx2: bool,
    pub fma: bool,
}

#[cfg(target_arch = "x86_64")]
static FEATURES: OnceLock<X86Features> = OnceLock::new();

#[cfg(target_arch = "x86_64")]
#[inline]
pub fn get() -> X86Features {
    *FEATURES.get_or_init(|| X86Features {
        sse2: is_x86_feature_detected!("sse2"),
        sse3: is_x86_feature_detected!("sse3"),
        ssse3: is_x86_feature_detected!("ssse3"),
        sse4_1: is_x86_feature_detected!("sse4.1"),
        avx2: is_x86_feature_detected!("avx2"),
        fma: is_x86_feature_detected!("fma"),
    })
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn get() -> X86Features {
    X86Features {
        sse2: false,
        sse3: false,
        ssse3: false,
        sse4_1: false,
        avx2: false,
        fma: false,
    }
}

#[inline]
pub fn has_sse2() -> bool {
    get().sse2
}

#[inline]
pub fn has_sse3() -> bool {
    get().sse3
}

#[inline]
pub fn has_ssse3() -> bool {
    get().ssse3
}

#[inline]
pub fn has_sse4_1() -> bool {
    get().sse4_1
}

#[inline]
pub fn has_avx2() -> bool {
    get().avx2
}

#[inline]
pub fn has_avx2_fma() -> bool {
    let f = get();
    f.avx2 && f.fma
}
