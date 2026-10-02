//! [`Backend`]: one SIMD backend of a kernel, as the cross-check sweeps it.

use crate::testing::simd_check::simd_tier::SimdTier;

/// One SIMD backend of a kernel: the tier it needs, the kernel itself (usually its function), and
/// the narrowest width its safety contract admits.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Backend<B> {
    pub(crate) tier: SimdTier,
    pub(crate) kernel: B,
    /// The sweep skips widths below this.
    pub(crate) min_width: usize,
}

impl<B: Copy> Backend<B> {
    pub(crate) const fn new(tier: SimdTier, kernel: B) -> Backend<B> {
        Backend {
            tier,
            kernel,
            min_width: 0,
        }
    }

    /// A backend whose safety contract admits no width below `min_width`.
    pub(crate) const fn with_min_width(tier: SimdTier, kernel: B, min_width: usize) -> Backend<B> {
        Backend {
            tier,
            kernel,
            min_width,
        }
    }

    /// The backends of `backends` the running CPU can execute, in order; each one it cannot is
    /// reported (see [`SimdTier::runs_here`]).
    pub(crate) fn supported(backends: &[Backend<B>]) -> impl Iterator<Item = Backend<B>> {
        backends
            .iter()
            .copied()
            .filter(|backend| backend.tier.runs_here())
    }
}
