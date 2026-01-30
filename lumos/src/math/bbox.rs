//! Axis-aligned bounding box for pixel regions.

use super::Vec2us;

/// Axis-aligned bounding box with `usize` coordinates.
///
/// Represents a rectangular region in pixel coordinates.
/// Uses inclusive bounds: a pixel at pos is inside if
/// `min.x <= pos.x <= max.x` and `min.y <= pos.y <= max.y`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Aabb {
    pub min: Vec2us,
    pub max: Vec2us,
}

#[allow(dead_code)] // Public API - used by tests and downstream code
impl Aabb {
    /// Create a new bounding box with the given bounds.
    #[inline]
    pub const fn new(min: Vec2us, max: Vec2us) -> Self {
        Self { min, max }
    }

    /// Create an empty bounding box (for accumulation).
    ///
    /// The empty box has inverted bounds so that any point
    /// included via `include()` will set the initial bounds.
    #[inline]
    pub const fn empty() -> Self {
        Self {
            min: Vec2us::new(usize::MAX, usize::MAX),
            max: Vec2us::ZERO,
        }
    }

    /// Expand this bounding box to include the given point.
    #[inline]
    pub fn include(&mut self, pos: Vec2us) {
        self.min.x = self.min.x.min(pos.x);
        self.max.x = self.max.x.max(pos.x);
        self.min.y = self.min.y.min(pos.y);
        self.max.y = self.max.y.max(pos.y);
    }

    /// Width of the bounding box (number of columns).
    #[inline]
    pub const fn width(&self) -> usize {
        self.max.x.saturating_sub(self.min.x) + 1
    }

    /// Height of the bounding box (number of rows).
    #[inline]
    pub const fn height(&self) -> usize {
        self.max.y.saturating_sub(self.min.y) + 1
    }

    /// Check if a point is inside the bounding box.
    #[inline]
    pub const fn contains(&self, pos: Vec2us) -> bool {
        pos.x >= self.min.x && pos.x <= self.max.x && pos.y >= self.min.y && pos.y <= self.max.y
    }

    /// Area of the bounding box (width * height).
    #[inline]
    pub const fn area(&self) -> usize {
        self.width() * self.height()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let bbox = Aabb::new(Vec2us::new(1, 2), Vec2us::new(5, 8));
        assert_eq!(bbox.min.x, 1);
        assert_eq!(bbox.max.x, 5);
        assert_eq!(bbox.min.y, 2);
        assert_eq!(bbox.max.y, 8);
    }

    #[test]
    fn test_empty() {
        let bbox = Aabb::empty();
        assert_eq!(bbox.min.x, usize::MAX);
        assert_eq!(bbox.max.x, 0);
        assert_eq!(bbox.min.y, usize::MAX);
        assert_eq!(bbox.max.y, 0);
    }

    #[test]
    fn test_include() {
        let mut bbox = Aabb::empty();
        bbox.include(Vec2us::new(5, 3));
        assert_eq!(bbox, Aabb::new(Vec2us::new(5, 3), Vec2us::new(5, 3)));

        bbox.include(Vec2us::new(2, 7));
        assert_eq!(bbox, Aabb::new(Vec2us::new(2, 3), Vec2us::new(5, 7)));

        bbox.include(Vec2us::new(8, 1));
        assert_eq!(bbox, Aabb::new(Vec2us::new(2, 1), Vec2us::new(8, 7)));
    }

    #[test]
    fn test_width_height() {
        let bbox = Aabb::new(Vec2us::new(2, 3), Vec2us::new(5, 8));
        assert_eq!(bbox.width(), 4); // 5 - 2 + 1
        assert_eq!(bbox.height(), 6); // 8 - 3 + 1
    }

    #[test]
    fn test_single_pixel() {
        let bbox = Aabb::new(Vec2us::new(3, 5), Vec2us::new(3, 5));
        assert_eq!(bbox.width(), 1);
        assert_eq!(bbox.height(), 1);
        assert_eq!(bbox.area(), 1);
    }

    #[test]
    fn test_contains() {
        let bbox = Aabb::new(Vec2us::new(2, 3), Vec2us::new(5, 8));
        assert!(bbox.contains(Vec2us::new(2, 3))); // corner
        assert!(bbox.contains(Vec2us::new(5, 8))); // corner
        assert!(bbox.contains(Vec2us::new(3, 5))); // inside
        assert!(!bbox.contains(Vec2us::new(1, 5))); // left of
        assert!(!bbox.contains(Vec2us::new(6, 5))); // right of
        assert!(!bbox.contains(Vec2us::new(3, 2))); // above
        assert!(!bbox.contains(Vec2us::new(3, 9))); // below
    }

    #[test]
    fn test_area() {
        let bbox = Aabb::new(Vec2us::new(0, 0), Vec2us::new(9, 4));
        assert_eq!(bbox.area(), 50); // 10 * 5
    }
}
