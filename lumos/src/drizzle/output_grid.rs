//! [`OutputGrid`]: the grid a drizzle run reconstructs onto, `scale` times finer than its
//! reference.

use glam::DVec2;

use crate::registration::transform::Transform;
use common::Size2us;

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
    size: Size2us,
}

impl OutputGrid {
    /// The grid an `input` frame drizzles onto at `scale`: each side `scale` times the input's,
    /// rounded up so every drop of a validated config lands inside it.
    #[expect(
        clippy::cast_sign_loss,
        reason = "a validated config holds the scale positive, so each side is non-negative"
    )]
    pub(crate) fn new(scale: f32, input: Size2us) -> Self {
        debug_assert!(scale > 0.0, "a drizzle scale is positive, not {scale}");
        Self {
            scale: f64::from(scale),
            size: Size2us::new(
                (input.width as f32 * scale).ceil() as usize,
                (input.height as f32 * scale).ceil() as usize,
            ),
        }
    }

    pub(crate) const fn size(self) -> Size2us {
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
