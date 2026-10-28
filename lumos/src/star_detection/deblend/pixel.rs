//! [`Pixel`]: one pixel of a component, where it is and what it holds.

use std::cmp::Ordering;

use glam::{USizeVec2, UVec2};

/// A pixel with its coordinates and value: 12 bytes, which a deblend holds one of for each pixel
/// of the component it splits. A frame's sides fit a `u32`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pixel {
    pos: UVec2,
    pub(crate) value: f32,
}

impl Pixel {
    /// The pixel at column `x` of row `y`, as a labeling run states them.
    pub(crate) const fn at(x: u32, y: u32, value: f32) -> Self {
        Self {
            pos: UVec2::new(x, y),
            value,
        }
    }

    pub(crate) const fn pos(&self) -> USizeVec2 {
        USizeVec2::new(self.pos.x as usize, self.pos.y as usize)
    }

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

#[cfg(test)]
pub(crate) mod internals {
    use crate::star_detection::deblend::pixel::Pixel;
    use glam::USizeVec2;

    impl Pixel {
        /// The pixel at `pos`.
        pub(crate) fn new(pos: USizeVec2, value: f32) -> Self {
            Self::at(
                u32::try_from(pos.x).unwrap(),
                u32::try_from(pos.y).unwrap(),
                value,
            )
        }
    }
}
