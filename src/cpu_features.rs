//! CPU feature detection for runtime SIMD dispatch.
//!
//! Cached on first call; use these instead of `is_x86_feature_detected!` directly. SSE2 is part
//! of the `x86_64` baseline, so it is not a feature to detect.

#[cfg(target_arch = "x86_64")]
use std::sync::OnceLock;

/// The `x86_64` features the dispatch tables test.
#[cfg(target_arch = "x86_64")]
#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per instruction set, each tested on its own"
)]
pub(crate) struct X86Features {
    pub(crate) ssse3: bool,
    pub(crate) sse4_1: bool,
    pub(crate) avx2: bool,
    fma: bool,
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub(crate) fn get() -> X86Features {
    static FEATURES: OnceLock<X86Features> = OnceLock::new();
    *FEATURES.get_or_init(|| X86Features {
        ssse3: is_x86_feature_detected!("ssse3"),
        sse4_1: is_x86_feature_detected!("sse4.1"),
        avx2: is_x86_feature_detected!("avx2"),
        fma: is_x86_feature_detected!("fma"),
    })
}

#[inline]
pub fn has_sse4_1() -> bool {
    #[cfg(target_arch = "x86_64")]
    return get().sse4_1;
    #[cfg(not(target_arch = "x86_64"))]
    false
}

#[inline]
pub fn has_avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    return get().avx2;
    #[cfg(not(target_arch = "x86_64"))]
    false
}

#[inline]
pub fn has_avx2_fma() -> bool {
    #[cfg(target_arch = "x86_64")]
    return get().avx2 && get().fma;
    #[cfg(not(target_arch = "x86_64"))]
    false
}

#[cfg(test)]
mod tests {
    use crate::cpu_features;

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn cached_features_match_runtime_detection() {
        let features = cpu_features::get();
        assert_eq!(features.ssse3, is_x86_feature_detected!("ssse3"));
        assert_eq!(features.sse4_1, is_x86_feature_detected!("sse4.1"));
        assert_eq!(features.avx2, is_x86_feature_detected!("avx2"));
        assert_eq!(features.fma, is_x86_feature_detected!("fma"));
        assert_eq!(cpu_features::has_sse4_1(), features.sse4_1);
        assert_eq!(cpu_features::has_avx2(), features.avx2);
        assert_eq!(cpu_features::has_avx2_fma(), features.avx2 && features.fma);
    }

    #[cfg(not(target_arch = "x86_64"))]
    #[test]
    fn non_x86_features_are_all_disabled() {
        assert!(!cpu_features::has_sse4_1());
        assert!(!cpu_features::has_avx2());
        assert!(!cpu_features::has_avx2_fma());
    }
}
