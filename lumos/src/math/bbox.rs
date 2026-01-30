//! Axis-aligned bounding box for pixel regions.

/// Axis-aligned bounding box with `usize` coordinates.
///
/// Represents a rectangular region in pixel coordinates.
/// Uses inclusive bounds: a pixel at (x, y) is inside if
/// `x_min <= x <= x_max` and `y_min <= y <= y_max`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Aabb {
    pub x_min: usize,
    pub x_max: usize,
    pub y_min: usize,
    pub y_max: usize,
}

#[allow(dead_code)] // Public API - used by tests and downstream code
impl Aabb {
    /// Create a new bounding box with the given bounds.
    #[inline]
    pub const fn new(x_min: usize, x_max: usize, y_min: usize, y_max: usize) -> Self {
        Self {
            x_min,
            x_max,
            y_min,
            y_max,
        }
    }

    /// Create an empty bounding box (for accumulation).
    ///
    /// The empty box has inverted bounds so that any point
    /// included via `include()` will set the initial bounds.
    #[inline]
    pub const fn empty() -> Self {
        Self {
            x_min: usize::MAX,
            x_max: 0,
            y_min: usize::MAX,
            y_max: 0,
        }
    }

    /// Expand this bounding box to include the given point.
    #[inline]
    pub fn include(&mut self, x: usize, y: usize) {
        self.x_min = self.x_min.min(x);
        self.x_max = self.x_max.max(x);
        self.y_min = self.y_min.min(y);
        self.y_max = self.y_max.max(y);
    }

    /// Width of the bounding box (number of columns).
    #[inline]
    pub const fn width(&self) -> usize {
        self.x_max.saturating_sub(self.x_min) + 1
    }

    /// Height of the bounding box (number of rows).
    #[inline]
    pub const fn height(&self) -> usize {
        self.y_max.saturating_sub(self.y_min) + 1
    }

    /// Check if a point is inside the bounding box.
    #[inline]
    pub const fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x_min && x <= self.x_max && y >= self.y_min && y <= self.y_max
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
        let bbox = Aabb::new(1, 5, 2, 8);
        assert_eq!(bbox.x_min, 1);
        assert_eq!(bbox.x_max, 5);
        assert_eq!(bbox.y_min, 2);
        assert_eq!(bbox.y_max, 8);
    }

    #[test]
    fn test_empty() {
        let bbox = Aabb::empty();
        assert_eq!(bbox.x_min, usize::MAX);
        assert_eq!(bbox.x_max, 0);
        assert_eq!(bbox.y_min, usize::MAX);
        assert_eq!(bbox.y_max, 0);
    }

    #[test]
    fn test_include() {
        let mut bbox = Aabb::empty();
        bbox.include(5, 3);
        assert_eq!(bbox, Aabb::new(5, 5, 3, 3));

        bbox.include(2, 7);
        assert_eq!(bbox, Aabb::new(2, 5, 3, 7));

        bbox.include(8, 1);
        assert_eq!(bbox, Aabb::new(2, 8, 1, 7));
    }

    #[test]
    fn test_width_height() {
        let bbox = Aabb::new(2, 5, 3, 8);
        assert_eq!(bbox.width(), 4); // 5 - 2 + 1
        assert_eq!(bbox.height(), 6); // 8 - 3 + 1
    }

    #[test]
    fn test_single_pixel() {
        let bbox = Aabb::new(3, 3, 5, 5);
        assert_eq!(bbox.width(), 1);
        assert_eq!(bbox.height(), 1);
        assert_eq!(bbox.area(), 1);
    }

    #[test]
    fn test_contains() {
        let bbox = Aabb::new(2, 5, 3, 8);
        assert!(bbox.contains(2, 3)); // corner
        assert!(bbox.contains(5, 8)); // corner
        assert!(bbox.contains(3, 5)); // inside
        assert!(!bbox.contains(1, 5)); // left of
        assert!(!bbox.contains(6, 5)); // right of
        assert!(!bbox.contains(3, 2)); // above
        assert!(!bbox.contains(3, 9)); // below
    }

    #[test]
    fn test_area() {
        let bbox = Aabb::new(0, 9, 0, 4);
        assert_eq!(bbox.area(), 50); // 10 * 5
    }
}
