//! Pixel extent of a 2D grid.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use glam::USizeVec2;

/// How many columns and rows a 2D grid has.
///
/// Distinct from [`USizeVec2`], which locates a pixel: a size has no origin, adding two sizes is
/// meaningless, and its components are named for what they measure. Taking one instead of a
/// `width, height` pair also makes the two impossible to transpose at a call site — they are
/// both `usize`, so the compiler cannot otherwise tell them apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct USize2 {
    pub width: usize,
    pub height: usize,
}

impl USize2 {
    #[inline]
    pub const fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }

    /// Pixels the grid holds. Panics rather than wrapping: a count that overflows `usize` means
    /// the dimensions are nonsense, and every buffer sized from it would be too small.
    #[inline]
    pub const fn pixel_count(self) -> usize {
        self.width
            .checked_mul(self.height)
            .expect("grid pixel count must fit in usize")
    }

    /// Row-major index of `point`, for a buffer laid out by this size.
    ///
    /// # Panics
    /// Panics in debug builds if `point` is outside the grid.
    #[inline]
    pub const fn idx_of(self, point: USizeVec2) -> usize {
        debug_assert!(self.contains(point), "point is outside the grid");
        point.y * self.width + point.x
    }

    /// The point `idx` addresses — the inverse of [`Self::idx_of`].
    ///
    /// # Panics
    /// Panics in debug builds if `idx` is past the last pixel.
    #[inline]
    pub fn point_of(self, idx: usize) -> USizeVec2 {
        debug_assert!(
            idx < self.pixel_count(),
            "index {idx} past the last pixel of a {}x{} grid",
            self.width,
            self.height
        );
        USizeVec2::new(idx % self.width, idx / self.width)
    }

    /// Whether `point` lies inside the grid.
    #[inline]
    pub const fn contains(self, point: USizeVec2) -> bool {
        point.x < self.width && point.y < self.height
    }
}

impl Display for USize2 {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}x{}", self.width, self.height)
    }
}

impl From<(usize, usize)> for USize2 {
    #[inline]
    fn from((width, height): (usize, usize)) -> Self {
        Self { width, height }
    }
}

impl From<USize2> for (usize, usize) {
    #[inline]
    fn from(size: USize2) -> Self {
        (size.width, size.height)
    }
}

#[cfg(test)]
mod tests;
