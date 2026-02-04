//! 2D vector with `usize` components for pixel coordinates.

use std::ops::{Add, Sub};

/// 2D vector with `usize` components.
///
/// Unlike glam's vector types (which use `i32`, `u32`, or `f32`),
/// this uses `usize` for direct array indexing without casts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Vec2us {
    pub x: usize,
    pub y: usize,
}

impl Vec2us {
    /// Create a new vector.
    #[inline]
    pub const fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }

    /// Zero vector.
    pub const ZERO: Self = Self { x: 0, y: 0 };

    /// Compute linear index for row-major layout.
    #[inline]
    pub const fn to_index(self, width: usize) -> usize {
        self.y * width + self.x
    }

    /// Create from linear index with row-major layout.
    #[inline]
    pub const fn from_index(index: usize, width: usize) -> Self {
        Self {
            x: index % width,
            y: index / width,
        }
    }
}

impl Add for Vec2us {
    type Output = Self;

    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for Vec2us {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl From<(usize, usize)> for Vec2us {
    #[inline]
    fn from((x, y): (usize, usize)) -> Self {
        Self { x, y }
    }
}

impl From<Vec2us> for (usize, usize) {
    #[inline]
    fn from(v: Vec2us) -> Self {
        (v.x, v.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let v = Vec2us::new(3, 5);
        assert_eq!(v.x, 3);
        assert_eq!(v.y, 5);
    }

    #[test]
    fn test_to_index() {
        let v = Vec2us::new(3, 2);
        assert_eq!(v.to_index(10), 23); // 2 * 10 + 3
    }

    #[test]
    fn test_from_index() {
        let v = Vec2us::from_index(23, 10);
        assert_eq!(v, Vec2us::new(3, 2));
    }

    #[test]
    fn test_add() {
        let a = Vec2us::new(1, 2);
        let b = Vec2us::new(3, 4);
        assert_eq!(a + b, Vec2us::new(4, 6));
    }

    #[test]
    fn test_sub() {
        let a = Vec2us::new(5, 7);
        let b = Vec2us::new(2, 3);
        assert_eq!(a - b, Vec2us::new(3, 4));
    }

    #[test]
    fn test_from_tuple() {
        let v: Vec2us = (3, 5).into();
        assert_eq!(v, Vec2us::new(3, 5));
    }

    #[test]
    fn test_into_tuple() {
        let v = Vec2us::new(3, 5);
        let t: (usize, usize) = v.into();
        assert_eq!(t, (3, 5));
    }
}
