//! [`SimdTier`]: the instruction sets the SIMD kernels are compiled for.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
#[cfg(target_arch = "x86_64")]
use std::sync::OnceLock;

/// An instruction set a SIMD kernel is compiled for, detected once per process.
///
/// On one arch the tiers form a chain, narrowest first, and a tier counts as supported only when
/// every narrower one is too, so `tier >= SimdTier::Sse41` asks whether an SSE4.1 kernel may run.
/// Dispatch takes [`Self::widest`]. A test runs every supported tier, because on a wide CPU only
/// a test reaches the narrower kernels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SimdTier {
    /// The `x86_64` baseline.
    #[cfg(target_arch = "x86_64")]
    Sse2,
    #[cfg(target_arch = "x86_64")]
    Ssse3,
    #[cfg(target_arch = "x86_64")]
    Sse41,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    /// AVX2 with fused multiply-add, whose kernels round differently from the unfused ones.
    #[cfg(target_arch = "x86_64")]
    Avx2Fma,
    /// The aarch64 baseline.
    #[cfg(target_arch = "aarch64")]
    Neon,
}

impl SimdTier {
    /// Every tier of this arch, narrowest first.
    #[cfg(target_arch = "x86_64")]
    pub const ALL: [Self; 5] = [
        Self::Sse2,
        Self::Ssse3,
        Self::Sse41,
        Self::Avx2,
        Self::Avx2Fma,
    ];
    /// Every tier of this arch.
    #[cfg(target_arch = "aarch64")]
    pub const ALL: [Self; 1] = [Self::Neon];
    /// No tier: this arch has no SIMD kernels.
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    pub const ALL: [Self; 0] = [];

    /// The widest tier the running CPU has, or `None` on an arch with no kernels.
    #[inline]
    pub fn widest() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        {
            static WIDEST: OnceLock<SimdTier> = OnceLock::new();
            Some(*WIDEST.get_or_init(Self::detect))
        }
        #[cfg(target_arch = "aarch64")]
        return Some(Self::Neon);
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        None
    }

    /// Whether the running CPU has this tier and every narrower one.
    #[inline]
    pub fn is_supported(self) -> bool {
        Self::widest().is_some_and(|widest| self <= widest)
    }

    /// The last tier of the chain whose features, and every narrower tier's, the CPU has.
    #[cfg(target_arch = "x86_64")]
    fn detect() -> Self {
        let features = [
            is_x86_feature_detected!("ssse3"),
            is_x86_feature_detected!("sse4.1"),
            is_x86_feature_detected!("avx2"),
            is_x86_feature_detected!("fma"),
        ];
        let narrower_present = features.iter().take_while(|&&present| present).count();
        Self::ALL[narrower_present]
    }
}

impl Display for SimdTier {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            #[cfg(target_arch = "x86_64")]
            Self::Sse2 => "SSE2",
            #[cfg(target_arch = "x86_64")]
            Self::Ssse3 => "SSSE3",
            #[cfg(target_arch = "x86_64")]
            Self::Sse41 => "SSE4.1",
            #[cfg(target_arch = "x86_64")]
            Self::Avx2 => "AVX2",
            #[cfg(target_arch = "x86_64")]
            Self::Avx2Fma => "AVX2+FMA",
            #[cfg(target_arch = "aarch64")]
            Self::Neon => "NEON",
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::simd_tier::SimdTier;

    /// The detected tier is the chain's prefix of the features `std` detects, and every tier up
    /// to it is supported while none past it is.
    #[test]
    fn widest_is_the_supported_prefix_of_the_chain() {
        let widest = SimdTier::widest();
        #[cfg(target_arch = "x86_64")]
        {
            let mut expected = SimdTier::Sse2;
            for (feature, tier) in [
                (is_x86_feature_detected!("ssse3"), SimdTier::Ssse3),
                (is_x86_feature_detected!("sse4.1"), SimdTier::Sse41),
                (is_x86_feature_detected!("avx2"), SimdTier::Avx2),
                (is_x86_feature_detected!("fma"), SimdTier::Avx2Fma),
            ] {
                if !feature {
                    break;
                }
                expected = tier;
            }
            assert_eq!(widest, Some(expected));
        }
        #[cfg(target_arch = "aarch64")]
        assert_eq!(widest, Some(SimdTier::Neon));
        for tier in SimdTier::ALL {
            assert_eq!(tier.is_supported(), Some(tier) <= widest, "{tier:?}");
        }
    }
}
