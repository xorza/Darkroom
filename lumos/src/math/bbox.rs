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

impl Aabb {
    /// Create a new bounding box with the given bounds.
    #[inline]
    #[allow(dead_code)] // Used in tests
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

    /// Check if this bounding box is empty (no points included).
    #[inline]
    #[allow(dead_code)] // Used in tests
    pub const fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    /// Expand this bounding box to include the given point.
    #[inline]
    pub fn include(&mut self, pos: Vec2us) {
        self.min.x = self.min.x.min(pos.x);
        self.max.x = self.max.x.max(pos.x);
        self.min.y = self.min.y.min(pos.y);
        self.max.y = self.max.y.max(pos.y);
    }

    /// Width of the bounding box (number of columns). Returns 0 for empty boxes.
    #[inline]
    #[allow(dead_code)] // Used in tests
    pub const fn width(&self) -> usize {
        if self.is_empty() {
            return 0;
        }
        self.max.x - self.min.x + 1
    }

    /// Height of the bounding box (number of rows). Returns 0 for empty boxes.
    #[inline]
    #[allow(dead_code)] // Used in tests
    pub const fn height(&self) -> usize {
        if self.is_empty() {
            return 0;
        }
        self.max.y - self.min.y + 1
    }

    /// Check if a point is inside the bounding box.
    #[inline]
    #[allow(dead_code)] // Used in tests
    pub const fn contains(&self, pos: Vec2us) -> bool {
        pos.x >= self.min.x && pos.x <= self.max.x && pos.y >= self.min.y && pos.y <= self.max.y
    }

    /// Area of the bounding box (width * height).
    #[inline]
    #[allow(dead_code)] // Used in tests
    pub const fn area(&self) -> usize {
        self.width() * self.height()
    }

    /// Merge two bounding boxes, returning a box that contains both.
    #[inline]
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            min: Vec2us::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Vec2us::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
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
        assert!(bbox.is_empty());
        assert_eq!(bbox.width(), 0);
        assert_eq!(bbox.height(), 0);
        assert_eq!(bbox.area(), 0);
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
        assert!(!bbox.is_empty());
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

    #[test]
    fn test_merge_overlapping() {
        // Two overlapping boxes:
        //   A: (2,3) to (5,8) — width=4, height=6
        //   B: (4,6) to (9,10) — width=6, height=5
        // Merged: min=(2,3), max=(9,10) — width=8, height=8
        let a = Aabb::new(Vec2us::new(2, 3), Vec2us::new(5, 8));
        let b = Aabb::new(Vec2us::new(4, 6), Vec2us::new(9, 10));
        let merged = a.merge(&b);
        assert_eq!(merged.min, Vec2us::new(2, 3));
        assert_eq!(merged.max, Vec2us::new(9, 10));
        assert_eq!(merged.width(), 8); // 9 - 2 + 1
        assert_eq!(merged.height(), 8); // 10 - 3 + 1
    }

    #[test]
    fn test_merge_disjoint() {
        // Two disjoint boxes:
        //   A: (0,0) to (2,2)
        //   B: (10,10) to (12,12)
        // Merged: min=(0,0), max=(12,12)
        let a = Aabb::new(Vec2us::new(0, 0), Vec2us::new(2, 2));
        let b = Aabb::new(Vec2us::new(10, 10), Vec2us::new(12, 12));
        let merged = a.merge(&b);
        assert_eq!(merged.min, Vec2us::new(0, 0));
        assert_eq!(merged.max, Vec2us::new(12, 12));
        assert_eq!(merged.area(), 169); // 13 * 13
    }

    #[test]
    fn test_merge_identical() {
        let a = Aabb::new(Vec2us::new(3, 5), Vec2us::new(7, 9));
        let merged = a.merge(&a);
        assert_eq!(merged, a);
    }

    #[test]
    fn test_merge_contained() {
        // B is entirely inside A
        let a = Aabb::new(Vec2us::new(0, 0), Vec2us::new(10, 10));
        let b = Aabb::new(Vec2us::new(3, 3), Vec2us::new(7, 7));
        let merged = a.merge(&b);
        assert_eq!(merged, a);
    }

    #[test]
    fn test_merge_with_empty() {
        let a = Aabb::new(Vec2us::new(2, 3), Vec2us::new(5, 8));
        let empty = Aabb::empty();
        // Merging with empty: min takes min of (2, usize::MAX)=2, max takes max of (5, 0)=5
        // So for a non-empty box, merge with empty returns the non-empty box
        let merged = a.merge(&empty);
        assert_eq!(merged.min, Vec2us::new(2, 3));
        assert_eq!(merged.max, Vec2us::new(5, 8));
    }

    #[test]
    fn test_merge_is_commutative() {
        let a = Aabb::new(Vec2us::new(1, 2), Vec2us::new(4, 6));
        let b = Aabb::new(Vec2us::new(3, 5), Vec2us::new(8, 9));
        assert_eq!(a.merge(&b), b.merge(&a));
    }
}
