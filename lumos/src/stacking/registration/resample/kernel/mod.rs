//! Image interpolation for sub-pixel resampling.
//!
//! Lanczos, bicubic, bilinear and nearest-neighbour kernels, each sampling at a [`SourcePosition`]
//! — a position already known to lie inside the source footprint, so the border is the caller's.

use std::sync::OnceLock;

use crate::math::lanczos;
use crate::math::size2us::Size2us;
use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::resample::source_position::SourcePosition;
use imaginarium::Buffer2;

// Lanczos LUT: 4096 samples/unit gives ~0.00024 precision.
// Lanczos3 LUT: 4096 * 3 * 4 bytes = 48KB (fits in L1 cache).
pub(super) const LANCZOS_LUT_RESOLUTION: usize = 4096;

/// The Lanczos kernels the warp offers, by their support radius `a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LanczosOrder {
    Two,
    Three,
    Four,
}

impl LanczosOrder {
    /// The Lanczos order `method` names, or `None` for a method that is not Lanczos.
    pub(super) const fn of(method: InterpolationMethod) -> Option<Self> {
        match method {
            InterpolationMethod::Lanczos2 => Some(Self::Two),
            InterpolationMethod::Lanczos3 => Some(Self::Three),
            InterpolationMethod::Lanczos4 => Some(Self::Four),
            InterpolationMethod::Nearest
            | InterpolationMethod::Bilinear
            | InterpolationMethod::Bicubic => None,
        }
    }

    /// The support radius `a`: `2a` taps per axis.
    pub(super) const fn a(self) -> usize {
        match self {
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
        }
    }

    /// The taps a window holds before its centre cell, `a − 1`, signed for window arithmetic.
    pub(super) const fn taps_before(self) -> i32 {
        match self {
            Self::Two => 1,
            Self::Three => 2,
            Self::Four => 3,
        }
    }

    /// This order's table, built on first use.
    pub(super) fn lut(self) -> &'static LanczosLut {
        static LUTS: [OnceLock<LanczosLut>; 3] =
            [OnceLock::new(), OnceLock::new(), OnceLock::new()];
        LUTS[self.a() - 2].get_or_init(|| LanczosLut::new(self.a()))
    }
}

#[derive(Debug)]
pub(super) struct LanczosLut {
    pub(super) values: Vec<f32>,
    a: usize,
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
        Self { values, a }
    }

    /// Lookup for a known non-negative distance within [0, a].
    ///
    /// The only form the warp paths need: they derive tap distances that are non-negative by
    /// construction, so the `abs()` and `>= a` branch a signed distance would need are the caller's
    /// guarantee instead. `internals::lookup` is the signed form, for oracles.
    #[inline(always)]
    #[expect(
        clippy::cast_sign_loss,
        reason = "the caller passes a non-negative distance, as the debug assertion checks"
    )]
    pub(super) fn lookup_positive(&self, abs_x: f32) -> f32 {
        debug_assert!(abs_x >= 0.0 && abs_x <= self.a as f32);
        let idx = (abs_x * LANCZOS_LUT_RESOLUTION as f32 + 0.5) as usize;
        unsafe { *self.values.get_unchecked(idx) }
    }

    /// The `SIZE = 2a` separable tap weights for fractional offset `frac`, taps at
    /// `floor − (a − 1) ..= floor + a`.
    ///
    /// The one distance convention every Lanczos reader shares — the row warp, its x86 gather, and
    /// the quality maps: `(a − 1 − i) + frac` below the centre and `(i + 1 − a) − frac` above it,
    /// both non-negative, which is what lets the lookup skip its sign handling.
    #[inline]
    pub(super) fn weights<const SIZE: usize>(&self, frac: f32) -> [f32; SIZE] {
        let a = SIZE / 2;
        debug_assert_eq!(a, self.a, "a {SIZE}-tap window reads the order-{a} table");
        let mut weights = [0.0f32; SIZE];
        for (i, weight) in weights.iter_mut().enumerate() {
            let distance = if i < a {
                (a - 1 - i) as f32 + frac
            } else {
                (i + 1 - a) as f32 - frac
            };
            *weight = self.lookup_positive(distance);
        }
        weights
    }
}

/// Bicubic kernel (Catmull-Rom, a = -0.5).
#[inline]
fn bicubic_kernel(x: f32) -> f32 {
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

/// The four 1-D bicubic tap weights for a fractional offset `f`, ordered for
/// taps at `floor - 1 ..= floor + 2`. Single source of truth shared by the
/// bicubic sampler and the coverage pass.
#[inline]
pub(super) fn bicubic_weights(f: f32) -> [f32; 4] {
    [
        bicubic_kernel(f + 1.0),
        bicubic_kernel(f),
        bicubic_kernel(f - 1.0),
        bicubic_kernel(f - 2.0),
    ]
}

/// Bilinear sample at `pos`, held to the pixel-centre grid in the footprint's half-pixel rim.
#[inline]
#[expect(
    clippy::cast_sign_loss,
    reason = "a position clamped to the pixel-centre grid has non-negative cells"
)]
pub(super) fn bilinear_sample(input: &Buffer2<f32>, pos: SourcePosition) -> f32 {
    let size = Size2us::new(input.width(), input.height());
    let pos = pos.clamped_to_centers(size);
    let pixels = input.pixels();
    let x0 = pos.cell_x as usize;
    let y0 = pos.cell_y as usize;
    let x1 = (x0 + 1).min(size.width - 1);
    let y1 = (y0 + 1).min(size.height - 1);
    let row0 = y0 * size.width;
    let row1 = y1 * size.width;
    debug_assert!(row1 + x1 < pixels.len());
    // SAFETY: clamping to the pixel-centre grid puts `x0`, `y0` in the image, and `x1`, `y1` are
    // held to its last column and row.
    let (p00, p10, p01, p11) = unsafe {
        (
            *pixels.get_unchecked(row0 + x0),
            *pixels.get_unchecked(row0 + x1),
            *pixels.get_unchecked(row1 + x0),
            *pixels.get_unchecked(row1 + x1),
        )
    };

    let top = p00 + pos.fx * (p10 - p00);
    let bottom = p01 + pos.fx * (p11 - p01);
    top + pos.fy * (bottom - top)
}

/// The pixel nearest `pos`, a half rounding up as `f32::round` does.
#[inline]
#[expect(
    clippy::cast_sign_loss,
    reason = "a position clamped to the pixel-centre grid has non-negative cells"
)]
pub(super) fn nearest_sample(input: &Buffer2<f32>, pos: SourcePosition) -> f32 {
    let size = Size2us::new(input.width(), input.height());
    let pos = pos.clamped_to_centers(size);
    let x = pos.cell_x as usize + usize::from(pos.fx >= 0.5);
    let y = pos.cell_y as usize + usize::from(pos.fy >= 0.5);
    input.pixels()[y * size.width + x]
}

/// Catmull-Rom over the 4×4 window around `pos`, renormalized over the taps inside the source.
///
/// Dropping the out-of-bounds taps and dividing by the in-bounds weight gives edge pixels the true
/// in-bounds weighted average instead of darkening them by the missing taps; interior pixels are
/// unchanged, since bicubic weights sum to 1. The divisor cannot vanish: inside the footprint the
/// in-bounds weight per axis is least at its half-pixel rim, where the nearest tap weighs
/// `k(½)` = 0.5625 and the one beyond it `k(1½)` = −0.0625, so the window keeps at least ¼.
pub(super) fn bicubic_sample(data: &Buffer2<f32>, pos: SourcePosition) -> f32 {
    let wx = bicubic_weights(pos.fx);
    let wy = bicubic_weights(pos.fy);

    let (pixels, w, h) = (data.pixels(), data.width(), data.height());
    let mut sum = 0.0f32;
    let mut w_in = 0.0f32;
    for (py, &wyj) in (pos.cell_y - 1..).zip(&wy) {
        let Ok(py) = usize::try_from(py) else {
            continue;
        };
        if py >= h {
            continue;
        }
        let row_off = py * w;
        for (px, &wxi) in (pos.cell_x - 1..).zip(&wx) {
            let Ok(px) = usize::try_from(px) else {
                continue;
            };
            if px >= w {
                continue;
            }
            let weight = wxi * wyj;
            sum += pixels[row_off + px] * weight;
            w_in += weight;
        }
    }
    debug_assert!(w_in >= 0.25 - 1e-6, "in-bounds bicubic weight {w_in}");
    sum / w_in
}

#[cfg(test)]
pub(super) mod internals {
    use glam::DVec2;
    use imaginarium::Buffer2;

    use crate::math::size2us::Size2us;
    use crate::stacking::registration::config::{InterpolationMethod, WarpParams};
    use crate::stacking::registration::resample::kernel;
    use crate::stacking::registration::resample::kernel::{
        LANCZOS_LUT_RESOLUTION, LanczosLut, LanczosOrder,
    };
    use crate::stacking::registration::resample::source_position::SourcePosition;

    /// The signed-distance form of the table read, which only the oracles and the LUT
    /// bench need — the warp paths all go through [`LanczosLut::lookup_positive`].
    impl LanczosLut {
        /// The table read at a signed distance either side of centre, zero beyond the kernel's support.
        #[expect(
            clippy::cast_sign_loss,
            reason = "an absolute distance is non-negative"
        )]
        pub(crate) fn lookup(&self, x: f32) -> f32 {
            let abs_x = x.abs();
            if abs_x >= self.a as f32 {
                return 0.0;
            }
            let idx = (abs_x * LANCZOS_LUT_RESOLUTION as f32 + 0.5) as usize;
            self.values[idx]
        }
    }

    /// Lanczos-`a` at `pos` straight from the definition — every tap's signed distance looked up,
    /// the window summed and normalized — with the warp's own edge-extended bilinear where the
    /// window leaves the source.
    #[expect(
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        reason = "an image side is at most ImageDimensions::MAX_SIDE, 2^30, so a coordinate and a kernel's reach past it fit i32; the taps are read only once the window is inside the image"
    )]
    pub(crate) fn interpolate_lanczos(
        data: &Buffer2<f32>,
        pos: DVec2,
        order: LanczosOrder,
        border_value: f32,
    ) -> f32 {
        let size = Size2us::new(data.width(), data.height());
        let Some(position) = SourcePosition::within(pos, size) else {
            return border_value;
        };
        let a = order.a() as i32;
        let (x0, y0) = (position.cell_x, position.cell_y);
        // Taps run from `cell − (a − 1)` to `cell + a` inclusive.
        if x0 - a + 1 < 0
            || y0 - a + 1 < 0
            || x0 + a >= size.width as i32
            || y0 + a >= size.height as i32
        {
            return kernel::bilinear_sample(data, position);
        }

        let lut = order.lut();
        let pixels = data.pixels();
        let mut sum = 0.0f32;
        let mut total_x = 0.0f32;
        let mut total_y = 0.0f32;
        for i in -a + 1..=a {
            total_x += lut.lookup(position.fx - i as f32);
            total_y += lut.lookup(position.fy - i as f32);
        }
        for j in -a + 1..=a {
            let wy = lut.lookup(position.fy - j as f32);
            let row_off = (y0 + j) as usize * size.width;
            for i in -a + 1..=a {
                let wx = lut.lookup(position.fx - i as f32);
                sum += pixels[row_off + (x0 + i) as usize] * wx * wy;
            }
        }
        sum / (total_x * total_y)
    }

    /// Any method at `pos`, the border outside the footprint: the single-point oracle the row warp
    /// is held to.
    pub(crate) fn interpolate(data: &Buffer2<f32>, pos: DVec2, params: WarpParams) -> f32 {
        let size = Size2us::new(data.width(), data.height());
        let Some(position) = SourcePosition::within(pos, size) else {
            return params.border_value;
        };
        match params.method {
            InterpolationMethod::Nearest => kernel::nearest_sample(data, position),
            InterpolationMethod::Bilinear => kernel::bilinear_sample(data, position),
            InterpolationMethod::Bicubic => kernel::bicubic_sample(data, position),
            method => interpolate_lanczos(
                data,
                pos,
                LanczosOrder::of(method).expect("the remaining methods are Lanczos"),
                params.border_value,
            ),
        }
    }
}

#[cfg(test)]
mod tests;
