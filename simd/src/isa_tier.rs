//! [`IsaTier`]: the Isas this architecture has, as one value to dispatch on and to test each of.

#[cfg(target_arch = "x86_64")]
use crate::backend::avx2_fma::Avx2Fma;
#[cfg(target_arch = "aarch64")]
use crate::backend::neon::Neon;
#[cfg(any(
    feature = "internals",
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
use crate::backend::portable::Portable;
use crate::isa::Isa;
use crate::kernel::Kernel;

/// One Isa of this architecture, holding its token. An `IsaTier` exists only for an Isa this CPU
/// runs, so running a kernel on one needs no further check.
///
/// The one list of Isas: [`Kernel::dispatch`] takes [`IsaTier::widest`], and the tests walk every
/// tier the CPU has, so an Isa cannot be dispatched to without being tested.
#[derive(Debug, Clone, Copy)]
pub enum IsaTier {
    /// Dispatched only on an architecture with no hardware Isa; kept on the others for the tests
    /// and benches.
    #[cfg(any(
        feature = "internals",
        not(any(target_arch = "x86_64", target_arch = "aarch64"))
    ))]
    Portable(Portable),
    #[cfg(target_arch = "x86_64")]
    Avx2Fma(Avx2Fma),
    #[cfg(target_arch = "aarch64")]
    Neon(Neon),
}

impl IsaTier {
    /// The widest Isa this CPU has: its architecture's baseline.
    #[inline]
    pub const fn widest() -> Self {
        #[cfg(target_arch = "x86_64")]
        return Self::Avx2Fma(Avx2Fma::new());
        #[cfg(target_arch = "aarch64")]
        return Self::Neon(Neon::new());
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        Self::Portable(Portable::new())
    }

    /// `kernel` on this tier's Isa.
    #[inline]
    pub fn run<K: Kernel>(self, kernel: K) -> K::Output {
        match self {
            #[cfg(any(
                feature = "internals",
                not(any(target_arch = "x86_64", target_arch = "aarch64"))
            ))]
            Self::Portable(isa) => isa.run(kernel),
            #[cfg(target_arch = "x86_64")]
            Self::Avx2Fma(isa) => isa.run(kernel),
            #[cfg(target_arch = "aarch64")]
            Self::Neon(isa) => isa.run(kernel),
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::fmt;
    use std::fmt::{Display, Formatter};
    use std::iter;

    use crate::backend::portable::Portable;
    use crate::isa_tier::IsaTier;

    impl IsaTier {
        /// `Portable`, the tier every other one is held to.
        pub const fn portable() -> Self {
            Self::Portable(Portable::new())
        }

        /// Every tier this CPU runs: `Portable`, then its architecture's baseline where it has one.
        pub fn supported() -> impl Iterator<Item = Self> {
            #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
            let baseline = Some(Self::widest());
            #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
            let baseline = None;
            iter::once(Self::portable()).chain(baseline)
        }

        /// The tier's part of a bench id: `portable`, `avx2_fma`, `neon`.
        pub const fn id(self) -> &'static str {
            match self {
                Self::Portable(_) => "portable",
                #[cfg(target_arch = "x86_64")]
                Self::Avx2Fma(_) => "avx2_fma",
                #[cfg(target_arch = "aarch64")]
                Self::Neon(_) => "neon",
            }
        }
    }

    impl Display for IsaTier {
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
