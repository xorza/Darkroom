//! [`SourcePosition`]: where an output pixel samples its source, split before it is narrowed.

use glam::DVec2;

use crate::math::size2us::Size2us;

/// A source position as its integer cell and the fraction within that cell.
///
/// The split is made in f64 and only the fraction is narrowed to f32. Narrowing the position first
/// would quantize it to the f32 spacing at its magnitude — 4.9e-4 px in [4096, 8192), coarser than
/// the 2.4e-4 step the Lanczos table resolves — while the fraction, in `[0, 1]`, keeps 6e-8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SourcePosition {
    pub(super) cell_x: i32,
    pub(super) cell_y: i32,
    /// `[0, 1]`: narrowing can round a fraction within 2⁻²⁵ of 1 up to exactly 1, which every
    /// kernel reads as the next tap's full weight.
    pub(super) fx: f32,
    pub(super) fy: f32,
}

impl SourcePosition {
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
        let cell_x = floor_inside(p.x);
        let cell_y = floor_inside(p.y);
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
}

/// `x.floor()` for a coordinate inside a footprint, so within `i32` range: truncation, then one
/// step down for a negative non-integer. `f64::floor` is a library call on the x86-64 baseline, and
/// this runs twice per output pixel.
#[inline]
fn floor_inside(x: f64) -> i32 {
    let truncated = x as i32;
    truncated - i32::from(x < f64::from(truncated))
}

/// One axis of [`SourcePosition::clamped_to_centers`].
#[derive(Debug, Clone, Copy)]
struct ClampedAxis {
    cell: i32,
    fraction: f32,
}

impl ClampedAxis {
    /// Inside the footprint the cell is at least −1 and at most `length − 1`, so the two clamps
    /// are the rim on either side.
    #[inline]
    const fn of(cell: i32, fraction: f32, length: usize) -> ClampedAxis {
        let last = length as i32 - 1;
        if cell < 0 {
            ClampedAxis {
                cell: 0,
                fraction: 0.0,
            }
        } else if cell >= last {
            ClampedAxis {
                cell: last,
                fraction: 0.0,
            }
        } else {
            ClampedAxis { cell, fraction }
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec2;

    use crate::math::size2us::Size2us;
    use crate::registration::resample::source_position::SourcePosition;

    const SIZE: Size2us = Size2us::new(10, 6);

    /// Inside the footprint a position splits into its floor and the exact remainder; the rim is
    /// inclusive, and anything past it or not finite is outside.
    #[test]
    fn within_splits_inside_the_footprint_only() {
        let split = |x, y| SourcePosition::within(DVec2::new(x, y), SIZE);
        assert_eq!(
            split(3.25, 4.75),
            Some(SourcePosition {
                cell_x: 3,
                cell_y: 4,
                fx: 0.25,
                fy: 0.75
            })
        );
        assert_eq!(
            split(-0.5, 5.5),
            Some(SourcePosition {
                cell_x: -1,
                cell_y: 5,
                fx: 0.5,
                fy: 0.5
            })
        );
        for (x, y) in [
            (-0.500_000_1, 1.0),
            (9.500_000_1, 1.0),
            (1.0, 5.500_000_1),
            (f64::NAN, 1.0),
            (1.0, f64::INFINITY),
        ] {
            assert_eq!(split(x, y), None, "({x}, {y})");
        }
    }

    /// The fraction keeps its precision where the position would not: at x = 6000.123, f32 spacing
    /// is 4.9e-4, so a position narrowed first lands on 6000.123047 and loses 4.7e-5 of its
    /// fraction, while the f64 split keeps 0.123 to f32's 7.5e-9 at that magnitude.
    #[test]
    fn the_fraction_is_split_before_it_is_narrowed() {
        let p = SourcePosition::within(DVec2::new(6000.123, 2.0), Size2us::new(8000, 4)).unwrap();
        assert_eq!(p.cell_x, 6000);
        assert!((f64::from(p.fx) - 0.123).abs() < 1e-8, "{}", p.fx);
        let narrowed_first = 6000.123f32 - 6000.0;
        assert!((f64::from(narrowed_first) - 0.123).abs() > 4e-5);
    }

    /// The rim on either side snaps to the edge pixel's centre; the interior is untouched.
    #[test]
    fn clamping_holds_positions_to_pixel_centres() {
        let clamp = |x, y| {
            SourcePosition::within(DVec2::new(x, y), SIZE)
                .unwrap()
                .clamped_to_centers(SIZE)
        };
        let at = |cell_x, fx, cell_y, fy| SourcePosition {
            cell_x,
            cell_y,
            fx,
            fy,
        };
        assert_eq!(clamp(-0.25, 9.0 - 4.0), at(0, 0.0, 5, 0.0));
        assert_eq!(clamp(9.25, 0.5), at(9, 0.0, 0, 0.5));
        assert_eq!(clamp(4.5, 2.25), at(4, 0.5, 2, 0.25));
    }
}
