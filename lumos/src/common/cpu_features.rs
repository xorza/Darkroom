//! CPU feature detection for runtime SIMD dispatch.

use std::sync::OnceLock;

/// CPU feature flags detected once at startup.
#[derive(Debug, Clone, Copy)]
pub struct X86Features {
    pub sse2: bool,
    pub sse3: bool,
    pub sse4_1: bool,
}

static FEATURES: OnceLock<X86Features> = OnceLock::new();

/// Get cached CPU features (detected once on first call).
#[inline]
pub fn get() -> X86Features {
    *FEATURES.get_or_init(|| X86Features {
        sse2: is_x86_feature_detected!("sse2"),
        sse3: is_x86_feature_detected!("sse3"),
        sse4_1: is_x86_feature_detected!("sse4.1"),
    })
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
