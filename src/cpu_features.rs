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
pub fn has_sse4_1() -> bool {
    get().sse4_1
}

#[inline]
pub fn has_avx2() -> bool {
    get().avx2
}

#[inline]
pub fn has_avx2_fma() -> bool {
    let features = get();
    features.avx2 && features.fma
}

#[cfg(test)]
mod tests {
    use crate::cpu_features;

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn cached_features_match_runtime_detection() {
        let features = cpu_features::get();

        assert_eq!(features.sse2, is_x86_feature_detected!("sse2"));
        assert_eq!(features.sse3, is_x86_feature_detected!("sse3"));
        assert_eq!(features.ssse3, is_x86_feature_detected!("ssse3"));
        assert_eq!(features.sse4_1, is_x86_feature_detected!("sse4.1"));
        assert_eq!(features.avx2, is_x86_feature_detected!("avx2"));
        assert_eq!(features.fma, is_x86_feature_detected!("fma"));
        assert_eq!(cpu_features::has_sse2(), features.sse2);
        assert_eq!(cpu_features::has_sse4_1(), features.sse4_1);
        assert_eq!(cpu_features::has_avx2(), features.avx2);
        assert_eq!(cpu_features::has_avx2_fma(), features.avx2 && features.fma);
    }

    #[cfg(not(target_arch = "x86_64"))]
    #[test]
    fn non_x86_features_are_all_disabled() {
        let features = cpu_features::get();

        assert!(!features.sse2);
        assert!(!features.sse3);
        assert!(!features.ssse3);
        assert!(!features.sse4_1);
        assert!(!features.avx2);
        assert!(!features.fma);
        assert!(!cpu_features::has_sse2());
        assert!(!cpu_features::has_sse4_1());
        assert!(!cpu_features::has_avx2());
        assert!(!cpu_features::has_avx2_fma());
    }
}
