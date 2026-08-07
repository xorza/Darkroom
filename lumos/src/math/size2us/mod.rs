//! Pixel extent of a 2D grid.

use crate::math::vec2us::Vec2us;

/// How many columns and rows a 2D grid has.
///
/// Distinct from [`Vec2us`], which locates a pixel: a size has no origin, adding two sizes is
/// meaningless, and its components are named for what they measure. Taking one instead of a
/// `width, height` pair also makes the two impossible to transpose at a call site — they are
/// both `usize`, so the compiler cannot otherwise tell them apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Size2us {
    pub width: usize,
    pub height: usize,
}

impl Size2us {
    #[inline]
    pub const fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }

    /// Pixels the grid holds. Panics rather than wrapping: a count that overflows `usize` means
    /// the dimensions are nonsense, and every buffer sized from it would be too small.
    #[inline]
    pub fn pixel_count(self) -> usize {
        self.width
            .checked_mul(self.height)
            .expect("grid pixel count must fit in usize")
    }

    /// Row-major index of `point`, for a buffer laid out by this size.
    ///
    /// # Panics
    /// Panics in debug builds if `point` is outside the grid.
    #[inline]
    pub const fn index_of(self, point: Vec2us) -> usize {
        debug_assert!(self.contains(point), "point is outside the grid");
        point.y * self.width + point.x
    }

    /// The point `index` addresses — the inverse of [`Self::index_of`].
    ///
    /// # Panics
    /// Panics in debug builds if `index` is past the last pixel.
    #[inline]
    pub fn point_of(self, index: usize) -> Vec2us {
        debug_assert!(
            index < self.pixel_count(),
            "index {index} past the last pixel of a {}x{} grid",
            self.width,
            self.height
        );
        Vec2us::new(index % self.width, index / self.width)
    }

    /// Whether `point` lies inside the grid.
    #[inline]
    pub const fn contains(self, point: Vec2us) -> bool {
        point.x < self.width && point.y < self.height
    }
}

impl From<(usize, usize)> for Size2us {
    #[inline]
    fn from((width, height): (usize, usize)) -> Self {
        Self { width, height }
    }
}

impl From<Size2us> for (usize, usize) {
    #[inline]
    fn from(size: Size2us) -> Self {
        (size.width, size.height)
    }
}

#[cfg(test)]
mod tests;
