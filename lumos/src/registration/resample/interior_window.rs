//! [`InteriorWindow`]: an output pixel's window held in registers, for the common case — wholly
//! inside the source, no null under it, and one vector of taps per axis.
//!
//! The sums are [`TapWindow`](crate::registration::resample::tap_window::TapWindow)'s for such a
//! window, from the same weights; only the order of their additions differs.

use crate::registration::resample::ringing_clamp::LobeSums;
use crate::registration::resample::source_image::SourcePlane;
use crate::registration::resample::source_pos::SourceAt;
use crate::registration::resample::tap_window::{AxisSums, Lobes, RowLanes, WindowWeights};
use crate::registration::resample::warp_kernel::WarpKernel;
use simd::{F32_LANES, F32x8, Isa, Mask8};

/// A window of at most [`F32_LANES`] taps per axis, wholly inside the source.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InteriorWindow<V: F32x8> {
    x: usize,
    y: usize,
    rows: usize,
    /// The x weights, cleared past the window's taps: a sum over the vector is a sum over them.
    wx: V,
    lanes: RowLanes<V>,
    wy: [f32; F32_LANES],
    weights: WindowWeights,
}

impl<V: F32x8> InteriorWindow<V> {
    /// The window of `kernel` at a position in its source; `None` when the kernel's taps there
    /// span more than a vector on either axis, or some tap lies outside the source.
    #[inline(always)]
    pub(crate) fn new<S: Isa<F32 = V>>(isa: S, kernel: &WarpKernel, at: SourceAt) -> Option<Self> {
        let SourceAt { pos, size } = at;
        let x = kernel.taps(pos.fx);
        let y = kernel.taps(pos.fy);
        if x.count > F32_LANES || y.count > F32_LANES {
            return None;
        }
        let first_x = usize::try_from(pos.cell_x + x.first).ok()?;
        let first_y = usize::try_from(pos.cell_y + y.first).ok()?;
        if first_x + x.count > size.width || first_y + y.count > size.height {
            return None;
        }
        let lanes = RowLanes::first(isa, x.count);
        let wx = lanes.keep().keep(kernel.weight_lanes(isa, x.first, pos.fx));
        let wy_lanes = RowLanes::first(isa, y.count)
            .keep()
            .keep(kernel.weight_lanes(isa, y.first, pos.fy));
        Some(Self {
            x: first_x,
            y: first_y,
            rows: y.count,
            wx,
            lanes,
            wy: wy_lanes.to_array(),
            weights: AxisSums::of_lanes(isa, wx).window(AxisSums::of_lanes(isa, wy_lanes)),
        })
    }

    /// The window's weight sums, as [`TapWindow::weights`] gives them for the same taps.
    ///
    /// [`TapWindow::weights`]: crate::registration::resample::tap_window::TapWindow::weights
    pub(crate) const fn weights(&self) -> WindowWeights {
        self.weights
    }

    /// `Σ L·f` over the window of `plane`.
    #[inline(always)]
    pub(crate) fn total<S: Isa<F32 = V>>(&self, isa: S, plane: SourcePlane<'_>) -> f32 {
        let mut sum = isa.splat_f32(0.0);
        for (j, &wy) in self.wy[..self.rows].iter().enumerate() {
            let on_row = self.lanes.load(isa, plane.pixels, self.row(plane, j)) * self.wx;
            sum = on_row.mul_add(isa.splat_f32(wy), sum);
        }
        sum.reduce_sum()
    }

    /// The window of `plane` summed as [`LobeSums`] reads it.
    #[inline(always)]
    pub(super) fn lobe_sums<S: Isa<F32 = V>>(&self, isa: S, plane: SourcePlane<'_>) -> LobeSums {
        let zero = isa.splat_f32(0.0);
        let (x_positive, x_negative) = (self.wx.max(zero), self.wx.min(zero));
        let mut lobes = Lobes::zero(isa);
        let mut below_zero = zero;
        for (j, &wy) in self.wy[..self.rows].iter().enumerate() {
            let values = self.lanes.load(isa, plane.pixels, self.row(plane, j));
            let light = values.max(zero);
            let on_row = Lobes {
                positive: light * x_positive,
                negative: light * x_negative,
            };
            lobes.add_row(isa, on_row, wy);
            below_zero = ((values - light) * self.wx).mul_add(isa.splat_f32(wy), below_zero);
        }
        LobeSums {
            positive: lobes.positive.reduce_sum(),
            negative: lobes.negative.reduce_sum(),
            below_zero: below_zero.reduce_sum(),
        }
    }

    #[inline(always)]
    const fn row(&self, plane: SourcePlane<'_>, j: usize) -> usize {
        (self.y + j) * plane.width + self.x
    }
}
