//! [`SourceMap`]: the input position an output pixel samples.

use glam::{Affine2, Vec2};

/// The output-to-input map of a warp: output pixel `(x, y)`, at its centre `(x + ½, y + ½)`,
/// samples the input at `(a · px + c · py) + tx − ½` across and `(b · px + d · py) + ty − ½`
/// down, each product and sum rounded apart in this order, as glam's `Affine2::transform_point2`
/// takes it. The vector kernels compute these same expressions lane by lane.
#[derive(Debug, Clone, Copy)]
pub(super) struct SourceMap {
    /// `(a, b)`: the input step of one output pixel across.
    pub(super) x_axis: Vec2,
    /// `(c, d)`: the input step of one output row down.
    pub(super) y_axis: Vec2,
    /// `(tx, ty)`.
    pub(super) translation: Vec2,
}

impl SourceMap {
    pub(super) fn new(inverse: Affine2) -> Self {
        Self {
            x_axis: inverse.matrix2.x_axis,
            y_axis: inverse.matrix2.y_axis,
            translation: inverse.translation,
        }
    }

    /// The input position output pixel `(x, y)` samples, in pixels from the first pixel's
    /// centre.
    #[inline]
    pub(super) fn position(self, x: usize, y: usize) -> Vec2 {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        Vec2::new(
            (self.x_axis.x * px + self.y_axis.x * py) + self.translation.x - 0.5,
            (self.x_axis.y * px + self.y_axis.y * py) + self.translation.y - 0.5,
        )
    }
}
