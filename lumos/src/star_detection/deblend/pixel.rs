//! [`Pixel`]: one pixel of a component, where it is and what it holds.

use std::cmp::Ordering;

use crate::math::vec2us::Vec2us;

/// A pixel with its coordinates and value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pixel {
    pub(crate) pos: Vec2us,
    pub(crate) value: f32,
}

impl Pixel {
    /// The brightest of `pixels`, the first in iteration order among equals; `None` when there
    /// are none.
    pub(super) fn brightest(pixels: impl IntoIterator<Item = Self>) -> Option<Self> {
        pixels
            .into_iter()
            .reduce(|best, p| if p.value > best.value { p } else { best })
    }

    /// Brightest first; equal values in raster order (row, then column), so a ranking does not
    /// depend on the order its input arrived in.
    pub(super) fn brighter_first(&self, other: &Self) -> Ordering {
        other
            .value
            .total_cmp(&self.value)
            .then(self.pos.y.cmp(&other.pos.y))
            .then(self.pos.x.cmp(&other.pos.x))
    }
}
