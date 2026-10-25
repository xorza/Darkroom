//! [`SourcePos`]: where an output pixel samples its source, split before it is narrowed.

use glam::DVec2;

use crate::math::floor_i32::FloorI32;
use common::Size2us;

/// A source position as its integer cell and the fraction within that cell.
///
/// The split is made in f64 and only the fraction is narrowed to f32. Narrowing the position first
/// would quantize it to the f32 spacing at its magnitude — 4.9e-4 px in [4096, 8192), coarser than
/// the 2.4e-4 step the Lanczos table resolves — while the fraction, in `[0, 1]`, keeps 6e-8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SourcePos {
    pub(super) cell_x: i32,
    pub(super) cell_y: i32,
    /// `[0, 1]`: narrowing can round a fraction within 2⁻²⁵ of 1 up to exactly 1, which every
    /// kernel reads as the next tap's full weight.
    pub(super) fx: f32,
    pub(super) fy: f32,
}

/// A sample's position along one axis: its integer cell, and the fraction within it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct AxisPos {
    pub(super) cell: i32,
    pub(super) frac: f32,
}

/// A sample's position in a source of `size`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SourceAt {
    pub(super) pos: SourcePos,
    pub(super) size: Size2us,
}

impl SourcePos {
    /// The position along x.
    #[inline(always)]
    pub(super) const fn x(self) -> AxisPos {
        AxisPos {
            cell: self.cell_x,
            frac: self.fx,
        }
    }

    /// The position along y.
    #[inline(always)]
    pub(super) const fn y(self) -> AxisPos {
        AxisPos {
            cell: self.cell_y,
            frac: self.fy,
        }
    }

    /// `p` split, when it lies in the footprint of a `size` source — `[-½, w − ½] × [-½, h − ½]`,
    /// the area its pixels cover; `None` outside it or for a non-finite `p`.
    #[inline]
    pub(super) fn within(p: DVec2, size: Size2us) -> Option<Self> {
        let max_x = size.width as f64 - 0.5;
        let max_y = size.height as f64 - 0.5;
        // Written so a NaN fails every comparison and lands outside.
        if !(p.x >= -0.5 && p.y >= -0.5 && p.x <= max_x && p.y <= max_y) {
            return None;
        }
        let cell_x = p.x.floor_i32();
        let cell_y = p.y.floor_i32();
        Some(Self {
            cell_x,
            cell_y,
            fx: (p.x - f64::from(cell_x)) as f32,
            fy: (p.y - f64::from(cell_y)) as f32,
        })
    }

    /// This position held to the grid of pixel centres, `[0, w − 1] × [0, h − 1]`: where bilinear
    /// and nearest sample a position in the half-pixel rim of the footprint.
    #[inline]
    pub(super) const fn clamped_to_centers(self, size: Size2us) -> Self {
        let x = ClampedAxis::of(self.cell_x, self.fx, size.width);
        let y = ClampedAxis::of(self.cell_y, self.fy, size.height);
        Self {
            cell_x: x.cell,
            cell_y: y.cell,
            fx: x.fraction,
            fy: y.fraction,
        }
    }

    /// The index of the pixel nearest this position in a `size` source, a half rounding up as
    /// `f32::round` does.
    #[inline]
    #[expect(
        clippy::cast_sign_loss,
        reason = "a position clamped to the pixel-centre grid has non-negative cells"
    )]
    pub(super) const fn nearest_idx(self, size: Size2us) -> usize {
        let pos = self.clamped_to_centers(size);
        let x = pos.cell_x as usize + (pos.fx >= 0.5) as usize;
        let y = pos.cell_y as usize + (pos.fy >= 0.5) as usize;
        y * size.width + x
    }
}

/// One axis of [`SourcePos::clamped_to_centers`].
#[derive(Debug, Clone, Copy)]
struct ClampedAxis {
    cell: i32,
    fraction: f32,
}

impl ClampedAxis {
    /// Inside the footprint the cell is at least −1 and at most `length − 1`, so the two clamps
    /// are the rim on either side.
    #[inline]
    const fn of(cell: i32, fraction: f32, length: usize) -> Self {
        let last = length as i32 - 1;
        if cell < 0 {
            Self {
                cell: 0,
                fraction: 0.0,
            }
        } else if cell >= last {
            Self {
                cell: last,
                fraction: 0.0,
            }
        } else {
            Self { cell, fraction }
        }
    }
}

#[cfg(test)]
mod tests;
