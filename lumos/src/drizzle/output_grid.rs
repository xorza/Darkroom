//! [`OutputGrid`]: the grid a drizzle run reconstructs onto, `scale` times finer than its
//! reference.

use glam::DVec2;

use crate::registration::transform::Transform;
use common::USize2;

/// The grid a drizzle run reconstructs onto: `scale` times finer than the reference grid, and
/// large enough for every drop of a validated config.
///
/// Integer-centred on both sides: reference pixel `p` spans `[p − ½, p + ½]` and output pixel `o`
/// spans `[o − ½, o + ½]`, so the reference footprint `[−½, w − ½]` has to land on `[−½, s·w − ½]`,
/// which is `o = s·p + (s − 1)/2`. A bare `o = s·p` would put the footprint at `[−s/2, s·w − s/2]`,
/// losing a strip of `(s − 1)/2` output pixels at the top and left of every frame and half-covering
/// the last row and column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OutputGrid {
    scale: f64,
    size: USize2,
}

impl OutputGrid {
    /// The grid an `input` frame drizzles onto at `scale`: through the last output pixel the
    /// reference footprint's far edge, `w − ½`, maps into by [`Self::reference_map`] — the same
    /// `f64` arithmetic, so the grid holds exactly the pixels the mapping reaches. A product `s·w`
    /// rounded on its own can land a column either side of it.
    #[expect(
        clippy::cast_sign_loss,
        reason = "a drizzle scale is positive, so each side is non-negative"
    )]
    pub(crate) fn new(scale: f64, input: USize2) -> Self {
        debug_assert!(scale > 0.0, "a drizzle scale is positive, not {scale}");
        let offset = (scale - 1.0) / 2.0;
        let side = |len: usize| (scale * (len as f64 - 0.5) + offset + 0.5).ceil() as usize;
        Self {
            scale,
            size: USize2::new(side(input.width), side(input.height)),
        }
    }

    pub(crate) const fn size(self) -> USize2 {
        self.size
    }

    /// The map from reference pixels onto the grid: `o = s·p + (s − 1)/2`.
    pub(super) fn reference_map(self) -> Transform {
        let offset = (self.scale - 1.0) / 2.0;
        Transform::affine([self.scale, 0.0, offset, 0.0, self.scale, offset])
    }

    /// The reference point output pixel `output` lies at: the inverse of
    /// [`Self::reference_map`].
    pub(crate) fn reference_point(self, output: DVec2) -> DVec2 {
        (output - (self.scale - 1.0) / 2.0) / self.scale
    }
}

#[cfg(test)]
mod tests {
    use common::USize2;
    use glam::DVec2;

    use crate::drizzle::output_grid::OutputGrid;

    /// The grid ends at the output pixel the last input pixel's far edge, `w − ½`, maps into: 100
    /// pixels at 1.5 reach 149.5, so 150 pixels; 7 at 0.5 reach 3.0, so 4; 50 at 1.1 reach 54.5,
    /// so 55, where `s·w` in `f64` is 55.00000000000001 and its ceiling grew a column no drop
    /// reaches; and 50 at 0.3 reach 14.5, so 15, where `f32` reads 15.000001.
    #[test]
    fn the_grid_holds_what_the_mapping_reaches() {
        for (width, scale, side) in [(100, 1.5, 150), (7, 0.5, 4), (50, 1.1, 55), (50, 0.3, 15)] {
            let grid = OutputGrid::new(scale, USize2::new(width, 1));
            assert_eq!(grid.size().width, side, "{width} at {scale}");
            let edge = grid
                .reference_map()
                .apply(DVec2::new(width as f64 - 0.5, 0.0))
                .x;
            assert!(edge <= side as f64 - 0.5, "{width} at {scale}: {edge}");
            assert!(edge > side as f64 - 1.5, "{width} at {scale}: {edge}");
        }
        assert_eq!((50.0 * 1.1_f64).ceil(), 56.0);
        assert_eq!((50.0f32 * 0.3f32).ceil(), 16.0);
    }
}
