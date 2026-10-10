//! The 1-D kernels a warp resamples through, and the nearest-neighbour read.
//!
//! [`WarpKernel`](warp_kernel::WarpKernel) turns a filter into one output pixel's tap weights, at
//! the frame's stretch; the table and the Catmull-Rom polynomial here are what it reads.

pub(super) mod warp_kernel;

use std::sync::OnceLock;

use crate::math::lanczos;
use crate::math::size2us::Size2us;
use crate::registration::resample::source_position::SourcePosition;

/// Entries per unit of distance. A read interpolates linearly between the two entries around the
/// distance, which is off the kernel by at most `max|L″|·h²/8`, under 4.5e-8 (see the table test),
/// and Lanczos4's table, `4·4096 + 1` entries, is 64 KiB.
pub(super) const LANCZOS_LUT_RESOLUTION: usize = 4096;

/// The Lanczos kernels the warp offers, by their support radius `a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LanczosOrder {
    Two,
    Three,
    Four,
}

impl LanczosOrder {
    /// The support radius `a`: `2a` taps per axis.
    pub(super) const fn a(self) -> usize {
        match self {
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
        }
    }

    /// This order's table, built on first use.
    pub(super) fn lut(self) -> &'static LanczosLut {
        static LUTS: [OnceLock<LanczosLut>; 3] =
            [OnceLock::new(), OnceLock::new(), OnceLock::new()];
        LUTS[self.a() - 2].get_or_init(|| LanczosLut::new(self.a()))
    }
}

/// Lanczos-`a` sampled every `1/RES` from 0 to `a`, the last entry the kernel's zero at `a`.
#[derive(Debug)]
pub(super) struct LanczosLut {
    pub(super) values: Vec<f32>,
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
}

/// The index of the pixel nearest `pos` in a `size` source, a half rounding up as `f32::round`
/// does.
#[inline]
#[expect(
    clippy::cast_sign_loss,
    reason = "a position clamped to the pixel-centre grid has non-negative cells"
)]
pub(super) fn nearest_index(size: Size2us, pos: SourcePosition) -> usize {
    let pos = pos.clamped_to_centers(size);
    let x = pos.cell_x as usize + usize::from(pos.fx >= 0.5);
    let y = pos.cell_y as usize + usize::from(pos.fy >= 0.5);
    y * size.width + x
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::registration::resample::kernel::LanczosLut;

    impl LanczosLut {
        /// The table at `scaled`, a non-negative distance already multiplied by the resolution:
        /// the line between the entries either side, and the kernel's zero at `a` past the
        /// table's end. The scalar read the vector gathers are held to.
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

    /// Catmull-Rom (`A = −½`), the definition the vector weights are held to.
    #[inline]
    pub(crate) fn bicubic_kernel(x: f32) -> f32 {
        const A: f32 = -0.5;
        let abs_x = x.abs();
        if abs_x <= 1.0 {
            ((A + 2.0) * abs_x - (A + 3.0)) * abs_x * abs_x + 1.0
        } else if abs_x < 2.0 {
            ((A * abs_x - 5.0 * A) * abs_x + 8.0 * A) * abs_x - 4.0 * A
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests;
