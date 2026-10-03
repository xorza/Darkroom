//! [`Tier`]: the Isas this architecture has, as one value to dispatch on and to test each of.

#[cfg(target_arch = "x86_64")]
use crate::simd::avx2_fma::Avx2Fma;
#[cfg(target_arch = "aarch64")]
use crate::simd::neon::Neon;
#[cfg(any(test, not(target_arch = "aarch64")))]
use crate::simd::portable::Portable;
use crate::simd::{Isa, Kernel};

/// One Isa of this architecture, holding its token. A `Tier` exists only for an Isa this CPU
/// runs, so running a kernel on one needs no further check.
///
/// The one list of Isas: [`Kernel::dispatch`] takes [`Tier::widest`], and the tests walk every
/// tier the CPU has, so an Isa cannot be dispatched to without being tested.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Tier {
    /// Never dispatched on aarch64, where every CPU has NEON; kept there for the tests.
    #[cfg(any(test, not(target_arch = "aarch64")))]
    Portable(Portable),
    #[cfg(target_arch = "x86_64")]
    Avx2Fma(Avx2Fma),
    #[cfg(target_arch = "aarch64")]
    Neon(Neon),
}

impl Tier {
    /// The widest Isa this CPU has.
    #[inline]
    #[cfg_attr(
        not(target_arch = "x86_64"),
        expect(
            clippy::missing_const_for_fn,
            reason = "only x86 detects at run time; the body is one shape on every arch"
        )
    )]
    pub(crate) fn widest() -> Self {
        #[cfg(target_arch = "x86_64")]
        if let Some(isa) = Avx2Fma::detect() {
            return Self::Avx2Fma(isa);
        }
        #[cfg(target_arch = "aarch64")]
        return Self::Neon(Neon::new());
        #[cfg(not(target_arch = "aarch64"))]
        Self::Portable(Portable::new())
    }

    /// `kernel` on this tier's Isa.
    #[inline]
    pub(crate) fn run<K: Kernel>(self, kernel: K) -> K::Output {
        match self {
            #[cfg(any(test, not(target_arch = "aarch64")))]
            Self::Portable(isa) => isa.run(kernel),
            #[cfg(target_arch = "x86_64")]
            Self::Avx2Fma(isa) => isa.run(kernel),
            #[cfg(target_arch = "aarch64")]
            Self::Neon(isa) => isa.run(kernel),
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::fmt;
    use std::fmt::{Display, Formatter};

    #[cfg(target_arch = "x86_64")]
    use crate::simd::avx2_fma::Avx2Fma;
    #[cfg(target_arch = "aarch64")]
    use crate::simd::neon::Neon;
    use crate::simd::portable::Portable;
    use crate::simd::tier::Tier;

    impl Tier {
        /// `Portable`, the tier every other one is held to.
        pub(crate) const fn portable() -> Self {
            Self::Portable(Portable::new())
        }

        /// Every tier this CPU runs, narrowest first. Each one it cannot is reported on stderr:
        /// a test has no skipped state, and would otherwise pass without checking it.
        pub(crate) fn supported() -> impl Iterator<Item = Tier> {
            let mut tiers = vec![Self::portable()];
            #[cfg(target_arch = "x86_64")]
            match Avx2Fma::detect() {
                Some(isa) => tiers.push(Self::Avx2Fma(isa)),
                None => {
                    eprintln!("SKIPPED: this CPU has no AVX2+FMA, so its kernels are not checked");
                }
            }
            #[cfg(target_arch = "aarch64")]
            tiers.push(Self::Neon(Neon::new()));
            tiers.into_iter()
        }
    }

    impl Display for Tier {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str(match self {
                Self::Portable(_) => "Portable",
                #[cfg(target_arch = "x86_64")]
                Self::Avx2Fma(_) => "AVX2+FMA",
                #[cfg(target_arch = "aarch64")]
                Self::Neon(_) => "NEON",
            })
        }
    }
}
