//! [`SimdTier`]: an instruction set a kernel has a backend for.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

#[cfg(target_arch = "x86_64")]
use imaginarium::cpu_features;

/// An instruction set a SIMD backend is compiled for, by the ISA token `dispatch!` names.
///
/// A cross-check runs every backend whose tier the host has, not only the one dispatch takes:
/// on an AVX2 host the SSE backends never run in production, so only a test can reach them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SimdTier {
    #[cfg(target_arch = "x86_64")]
    Sse2,
    #[cfg(target_arch = "x86_64")]
    Sse41,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Avx2Fma,
    #[cfg(target_arch = "aarch64")]
    Neon,
}

impl SimdTier {
    /// Whether the running CPU has this tier.
    pub(crate) fn is_supported(self) -> bool {
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::Sse2 => true,
            #[cfg(target_arch = "x86_64")]
            Self::Sse41 => cpu_features::has_sse4_1(),
            #[cfg(target_arch = "x86_64")]
            Self::Avx2 => cpu_features::has_avx2(),
            #[cfg(target_arch = "x86_64")]
            Self::Avx2Fma => cpu_features::has_avx2_fma(),
            #[cfg(target_arch = "aarch64")]
            Self::Neon => true,
        }
    }

    /// Whether a test can run this tier's backend here; when it cannot, the line on stderr says
    /// so, since a test has no skipped state and would otherwise pass without checking it.
    pub(crate) fn runs_here(self) -> bool {
        let supported = self.is_supported();
        if !supported {
            eprintln!("SKIPPED: this CPU has no {self}, so its backend is not checked");
        }
        supported
    }
}

impl Display for SimdTier {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            #[cfg(target_arch = "x86_64")]
            Self::Sse2 => "SSE2",
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
