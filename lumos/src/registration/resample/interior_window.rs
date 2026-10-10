//! [`InteriorWindow`]: an output pixel's window held in registers, for the common case — wholly
//! inside the source, no null under it, and one vector of taps per axis.
//!
//! The sums are [`TapWindow`](crate::registration::resample::tap_window::TapWindow)'s for such a
//! window, from the same weights; only the order of their additions differs.

use crate::math::size2us::Size2us;
use crate::registration::resample::kernel::warp_kernel::{TapRange, WarpKernel};
use crate::registration::resample::ringing_clamp::{LobeSums, LobeWeights};
use crate::registration::resample::source_image::SourcePlane;
use crate::registration::resample::source_position::SourcePosition;
use crate::registration::resample::tap_window::RowLanes;
use crate::simd::{F32_LANES, F32x8, Isa, Mask8};

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
    x_sums: AxisMoments,
    y_sums: AxisMoments,
}

/// One axis's weight sum and sum of squares.
#[derive(Debug, Clone, Copy)]
struct AxisMoments {
    total: f32,
    square: f32,
}

impl<V: F32x8> InteriorWindow<V> {
    /// The window of `kernel` at `position` over `x` and `y` taps of at most a vector each;
    /// `None` when some tap lies outside a `size` source.
    #[inline(always)]
    pub(crate) fn new<S: Isa<F32 = V>>(
        isa: S,
        kernel: &WarpKernel,
        position: SourcePosition,
        x: TapRange,
        y: TapRange,
        size: Size2us,
    ) -> Option<Self> {
        debug_assert!(x.count <= F32_LANES && y.count <= F32_LANES);
        let first_x = usize::try_from(position.cell_x + x.first).ok()?;
        let first_y = usize::try_from(position.cell_y + y.first).ok()?;
        if first_x + x.count > size.width || first_y + y.count > size.height {
            return None;
        }
        let lanes = RowLanes::first(isa, x.count);
        let wx = lanes
            .keep()
            .keep(kernel.weight_lanes(isa, x.first, position.fx));
        let wy_lanes = RowLanes::first(isa, y.count)
            .keep()
            .keep(kernel.weight_lanes(isa, y.first, position.fy));
        Some(Self {
            x: first_x,
            y: first_y,
            rows: y.count,
            wx,
            lanes,
            wy: wy_lanes.to_array(),
            x_sums: AxisMoments {
                total: wx.reduce_sum(),
                square: (wx * wx).reduce_sum(),
            },
            y_sums: AxisMoments {
                total: wy_lanes.reduce_sum(),
                square: (wy_lanes * wy_lanes).reduce_sum(),
            },
        })
    }

    /// `Σ L` over the window, `L = wₓ·w_y`.
    pub(crate) fn total_weight(&self) -> f32 {
        self.x_sums.total * self.y_sums.total
    }

    /// Kish's effective sample size of the normalized weights, as
    /// [`WindowWeights::confidence`](crate::registration::resample::tap_window::WindowWeights::confidence).
    pub(crate) fn confidence(&self) -> f32 {
        let total = self.total_weight();
        total * total / (self.x_sums.square * self.y_sums.square)
    }

    /// The window's weight by lobe, from each axis's split by sign.
    #[inline(always)]
    pub(crate) fn lobes<S: Isa<F32 = V>>(&self, isa: S) -> LobeWeights {
        let zero = isa.splat_f32(0.0);
        let wy = isa.load_f32(&self.wy);
        let (x_positive, x_negative) = (
            self.wx.max(zero).reduce_sum(),
            self.wx.min(zero).reduce_sum(),
        );
        let (y_positive, y_negative) = (wy.max(zero).reduce_sum(), wy.min(zero).reduce_sum());
        LobeWeights {
            positive: x_positive * y_positive + x_negative * y_negative,
            negative: x_positive * y_negative + x_negative * y_positive,
        }
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
    pub(crate) fn lobe_sums<S: Isa<F32 = V>>(&self, isa: S, plane: SourcePlane<'_>) -> LobeSums {
        let zero = isa.splat_f32(0.0);
        let (x_positive, x_negative) = (self.wx.max(zero), self.wx.min(zero));
        let (mut positive, mut negative, mut below_zero) = (zero, zero, zero);
        for (j, &wy) in self.wy[..self.rows].iter().enumerate() {
            let values = self.lanes.load(isa, plane.pixels, self.row(plane, j));
            let light = values.max(zero);
            let on_positive = light * x_positive;
            let on_negative = light * x_negative;
            let wy_lanes = isa.splat_f32(wy);
            let (to_positive, to_negative) = if wy > 0.0 {
                (on_positive, on_negative)
            } else {
                (on_negative, on_positive)
            };
            positive = to_positive.mul_add(wy_lanes, positive);
            negative = to_negative.mul_add(wy_lanes, negative);
            below_zero = ((values - light) * self.wx).mul_add(wy_lanes, below_zero);
        }
        LobeSums {
            positive: positive.reduce_sum(),
            negative: negative.reduce_sum(),
            below_zero: below_zero.reduce_sum(),
        }
    }

    #[inline(always)]
    const fn row(&self, plane: SourcePlane<'_>, j: usize) -> usize {
        (self.y + j) * plane.width + self.x
    }
}
