//! Half-open axis-aligned pixel rectangles.

use crate::vec2us::Vec2us;

// `Ord::min`/`max` are trait methods, which a `const fn` cannot call.
const fn min_usize(a: usize, b: usize) -> usize {
    if a < b { a } else { b }
}

const fn max_usize(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

/// Unsigned pixel rectangle with minimum-inclusive, maximum-exclusive bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect2us {
    pub min: Vec2us,
    pub max: Vec2us,
}

impl Rect2us {
    /// The rectangle from `min` to `max`. `min ≤ max` on both axes is the caller's contract,
    /// checked in debug builds, as a hot loop builds rectangles; the one with `min > max` is
    /// [`Self::empty`].
    #[inline]
    pub const fn new(min: Vec2us, max: Vec2us) -> Self {
        debug_assert!(min.x <= max.x && min.y <= max.y, "invalid rectangle bounds");
        Self { min, max }
    }

    /// The identity of [`Self::include`]: bounds inverted so that the first point included
    /// becomes the whole rectangle. It covers no pixel.
    #[inline]
    pub const fn empty() -> Self {
        Self {
            min: Vec2us::new(usize::MAX, usize::MAX),
            max: Vec2us::ZERO,
        }
    }

    // Saturating, not plain subtraction: `empty()` inverts the bounds to seed accumulation,
    // so it is the one rectangle whose min can exceed its max.
    #[inline]
    pub const fn width(self) -> usize {
        self.max.x.saturating_sub(self.min.x)
    }

    #[inline]
    pub const fn height(self) -> usize {
        self.max.y.saturating_sub(self.min.y)
    }

    /// Number of pixels the rectangle covers.
    #[inline]
    pub const fn area(self) -> usize {
        self.width() * self.height()
    }

    #[inline]
    pub const fn contains(self, point: Vec2us) -> bool {
        point.x >= self.min.x
            && point.x < self.max.x
            && point.y >= self.min.y
            && point.y < self.max.y
    }

    #[inline]
    pub const fn include(&mut self, point: Vec2us) {
        self.min.x = min_usize(self.min.x, point.x);
        self.min.y = min_usize(self.min.y, point.y);
        self.max.x = max_usize(self.max.x, point.x + 1);
        self.max.y = max_usize(self.max.y, point.y + 1);
    }
}

impl Default for Rect2us {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests;
