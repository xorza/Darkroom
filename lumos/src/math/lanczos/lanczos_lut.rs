//! [`LanczosLut`]: the Lanczos kernel tabulated, for the resamplers that read it per tap.

use std::sync::OnceLock;

use crate::math::lanczos;

/// Entries per unit of distance. A read interpolates linearly between the two entries around the
/// distance, which is off the kernel by at most `max|L″|·h²/8`, under 4.5e-8 (see the table test),
/// and Lanczos4's table, `4·4096 + 1` entries, is 64 KiB.
pub(crate) const LANCZOS_LUT_RESOLUTION: usize = 4096;

/// The Lanczos kernels the resamplers offer, by their support radius `a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LanczosOrder {
    Two,
    Three,
    Four,
}

impl LanczosOrder {
    /// The support radius `a`: `2a` taps per axis.
    pub(crate) const fn a(self) -> usize {
        match self {
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
        }
    }

    /// This order's table, built on first use.
    pub(crate) fn lut(self) -> &'static LanczosLut {
        static LUTS: [OnceLock<LanczosLut>; 3] =
            [OnceLock::new(), OnceLock::new(), OnceLock::new()];
        LUTS[self.a() - 2].get_or_init(|| LanczosLut::new(self.a()))
    }
}

/// Lanczos-`a` sampled every `1/RES` from 0 to `a`, the last entry the kernel's zero at `a`.
#[derive(Debug)]
pub(crate) struct LanczosLut {
    pub(crate) values: Vec<f32>,
}

impl LanczosLut {
    fn new(a: usize) -> Self {
        let num_entries = a * LANCZOS_LUT_RESOLUTION + 1;
        let a_f32 = a as f32;
        let values = (0..num_entries)
            .map(|i| {
                let x = i as f32 / LANCZOS_LUT_RESOLUTION as f32;
                lanczos::kernel(x, a_f32)
            })
            .collect();
        Self { values }
    }

    /// The table at `scaled`, a non-negative distance already multiplied by
    /// [`LANCZOS_LUT_RESOLUTION`]: the line between the entries either side, and the kernel's zero
    /// at `a` past the table's end. The scalar read the warp's vector gathers are held to.
    #[inline]
    #[expect(
        clippy::cast_sign_loss,
        reason = "the caller passes a non-negative distance, as the debug assertion checks"
    )]
    pub(crate) fn at(&self, scaled: f32) -> f32 {
        debug_assert!(scaled >= 0.0, "a table read at {scaled}");
        let last = self.values.len() - 1;
        let below = scaled.floor();
        let index = (below as usize).min(last);
        let low = self.values[index];
        let high = self.values[(index + 1).min(last)];
        (high - low).mul_add(scaled - below, low)
    }
}
