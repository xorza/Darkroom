//! [`PixelSize`]: an image's width and height, formatted on demand.

use std::fmt;
use std::fmt::{Display, Formatter};

use glam::UVec2;

/// An image's size in pixels, `4144 × 2822`, a space either side of the sign, as a dimension is
/// set in print. A preview's footer and the viewer's readout read it, so one size never
/// renders two ways.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PixelSize(pub(crate) UVec2);

impl Display for PixelSize {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} \u{d7} {}", self.0.x, self.0.y)
    }
}
