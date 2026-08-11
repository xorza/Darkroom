//! Half-open axis-aligned pixel rectangles.

use crate::math::vec2us::Vec2us;

// `usize::min`/`max` are not const-stable on the pinned toolchain.
const fn min_usize(a: usize, b: usize) -> usize {
    if a < b { a } else { b }
}

const fn max_usize(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

/// Unsigned pixel rectangle with minimum-inclusive, maximum-exclusive bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct URect {
    pub(crate) min: Vec2us,
    pub(crate) max: Vec2us,
}

impl URect {
    #[inline]
    pub(crate) const fn new(min: Vec2us, max: Vec2us) -> Self {
        assert!(min.x <= max.x && min.y <= max.y, "invalid rectangle bounds");
        Self { min, max }
    }

    #[inline]
    pub(crate) const fn empty() -> Self {
        Self {
            min: Vec2us::new(usize::MAX, usize::MAX),
            max: Vec2us::ZERO,
        }
    }

    #[inline]
    pub(crate) const fn is_empty(self) -> bool {
        self.min.x >= self.max.x || self.min.y >= self.max.y
    }

    // Saturating, not plain subtraction: `empty()` inverts the bounds to seed accumulation,
    // so it is the one rectangle whose min can exceed its max.
    #[inline]
    pub(crate) const fn width(self) -> usize {
        self.max.x.saturating_sub(self.min.x)
    }

    #[inline]
    pub(crate) const fn height(self) -> usize {
        self.max.y.saturating_sub(self.min.y)
    }

    /// Number of pixels the rectangle covers.
    #[inline]
    pub(crate) const fn area(self) -> usize {
        self.width() * self.height()
    }

    #[inline]
    pub(crate) const fn contains(self, point: Vec2us) -> bool {
        point.x >= self.min.x
            && point.x < self.max.x
            && point.y >= self.min.y
            && point.y < self.max.y
    }

    #[inline]
    pub(crate) const fn include(&mut self, point: Vec2us) {
        self.min.x = min_usize(self.min.x, point.x);
        self.min.y = min_usize(self.min.y, point.y);
        self.max.x = max_usize(self.max.x, point.x + 1);
        self.max.y = max_usize(self.max.y, point.y + 1);
    }

    #[inline]
    pub(crate) const fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }

        Self::new(
            Vec2us::new(
                min_usize(self.min.x, other.min.x),
                min_usize(self.min.y, other.min.y),
            ),
            Vec2us::new(
                max_usize(self.max.x, other.max.x),
                max_usize(self.max.y, other.max.y),
            ),
        )
    }
}

impl Default for URect {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests;
