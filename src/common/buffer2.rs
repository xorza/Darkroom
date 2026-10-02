//! `Buffer2<T>` — a generic 2D buffer over `Vec<T>` with `(x, y)`, linear, and
//! range indexing plus `Deref` to `[T]`. An interleaved image stores
//! `Buffer2<[T; N]>`; a planar one, one `Buffer2<T>` per channel, and
//! [`Buffer2::interleave`] / [`Buffer2::deinterleave`] move between the two.
//! Lumos uses it for `LinearImage` channel planes.

use std::ops::{Deref, DerefMut, Index, IndexMut, Range};
use std::{array, slice, vec};

use rayon::prelude::*;

/// `width × height` values in row-major order.
///
/// Every indexed access is bounds-checked in release against the whole buffer;
/// that `x < width` — which keeps a column from spilling into the next row — is
/// checked in debug builds, where a per-pixel accessor can afford it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer2<T> {
    pixels: Vec<T>,
    width: usize,
    height: usize,
}

/// `width · height`, or a panic naming the dimensions when it does not fit.
fn pixel_count(width: usize, height: usize) -> usize {
    width
        .checked_mul(height)
        .unwrap_or_else(|| panic!("{width}×{height} pixels do not fit in usize"))
}

impl<T> Buffer2<T> {
    pub fn new(width: usize, height: usize, pixels: Vec<T>) -> Self {
        assert_eq!(
            pixels.len(),
            pixel_count(width, height),
            "pixels length must equal width * height"
        );
        Self {
            pixels,
            width,
            height,
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> &T {
        &self[(x, y)]
    }

    #[inline]
    pub fn get_mut(&mut self, x: usize, y: usize) -> &mut T {
        &mut self[(x, y)]
    }

    #[inline]
    pub fn row(&self, y: usize) -> &[T] {
        &self.pixels[y * self.width..][..self.width]
    }

    #[inline]
    pub fn row_mut(&mut self, y: usize) -> &mut [T] {
        &mut self.pixels[y * self.width..][..self.width]
    }

    #[inline]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[inline]
    pub const fn height(&self) -> usize {
        self.height
    }

    #[inline]
    pub fn pixels(&self) -> &[T] {
        &self.pixels
    }

    #[inline]
    pub fn pixels_mut(&mut self) -> &mut [T] {
        &mut self.pixels
    }

    #[inline]
    pub fn into_vec(self) -> Vec<T> {
        self.pixels
    }
}

impl<T: Default + Clone> Buffer2<T> {
    pub fn new_default(width: usize, height: usize) -> Self {
        Self::new_filled(width, height, T::default())
    }
}

impl<T: Clone> Buffer2<T> {
    pub fn new_filled(width: usize, height: usize, value: T) -> Self {
        Self {
            pixels: vec![value; pixel_count(width, height)],
            width,
            height,
        }
    }
}

impl<T: Copy + Send + Sync, const N: usize> Buffer2<[T; N]> {
    /// Interleaves `N` channel planes into one pixel buffer.
    ///
    /// # Panics
    /// Unless every plane has the dimensions of the first.
    pub fn interleave(planes: [&Buffer2<T>; N]) -> Self {
        let (width, height) = (planes[0].width, planes[0].height);
        for plane in planes {
            assert_eq!(
                (plane.width, plane.height),
                (width, height),
                "all channel planes must share dimensions"
            );
        }
        let pixels = (0..width * height)
            .into_par_iter()
            .map(|i| planes.map(|plane| plane.pixels[i]))
            .collect();
        Self::new(width, height, pixels)
    }

    /// The `N` channel planes of this pixel buffer.
    pub fn deinterleave(&self) -> [Buffer2<T>; N] {
        array::from_fn(|channel| {
            let plane = self.pixels.par_iter().map(|pixel| pixel[channel]).collect();
            Buffer2::new(self.width, self.height, plane)
        })
    }
}

impl<T> Index<(usize, usize)> for Buffer2<T> {
    type Output = T;

    #[inline]
    fn index(&self, (x, y): (usize, usize)) -> &Self::Output {
        debug_assert!(x < self.width, "x {x} outside width {}", self.width);
        &self.pixels[y * self.width + x]
    }
}

impl<T> IndexMut<(usize, usize)> for Buffer2<T> {
    #[inline]
    fn index_mut(&mut self, (x, y): (usize, usize)) -> &mut Self::Output {
        debug_assert!(x < self.width, "x {x} outside width {}", self.width);
        &mut self.pixels[y * self.width + x]
    }
}

// These linear/range `Index` impls cannot be replaced by `Deref<[T]>`: once a
// type implements `Index` for any index, the `[]` operator commits to that type
// and never autoderefs to the slice's impls.
impl<T> Index<usize> for Buffer2<T> {
    type Output = T;

    #[inline]
    fn index(&self, idx: usize) -> &Self::Output {
        &self.pixels[idx]
    }
}

impl<T> IndexMut<usize> for Buffer2<T> {
    #[inline]
    fn index_mut(&mut self, idx: usize) -> &mut Self::Output {
        &mut self.pixels[idx]
    }
}

impl<T> Index<Range<usize>> for Buffer2<T> {
    type Output = [T];

    #[inline]
    fn index(&self, range: Range<usize>) -> &Self::Output {
        &self.pixels[range]
    }
}

impl<T> IndexMut<Range<usize>> for Buffer2<T> {
    #[inline]
    fn index_mut(&mut self, range: Range<usize>) -> &mut Self::Output {
        &mut self.pixels[range]
    }
}

impl<T> Deref for Buffer2<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.pixels
    }
}

impl<T> DerefMut for Buffer2<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pixels
    }
}

impl<'a, T> IntoIterator for &'a Buffer2<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.pixels.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Buffer2<T> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.pixels.iter_mut()
    }
}

impl<T> IntoIterator for Buffer2<T> {
    type Item = T;
    type IntoIter = vec::IntoIter<T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.pixels.into_iter()
    }
}

impl<T> From<Buffer2<T>> for Vec<T> {
    #[inline]
    fn from(buffer: Buffer2<T>) -> Self {
        buffer.pixels
    }
}

#[cfg(test)]
mod tests {
    use crate::common::buffer2::Buffer2;

    #[test]
    fn indexing_is_row_major() {
        let mut buf = Buffer2::new(3, 2, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!((buf.width(), buf.height(), buf.len()), (3, 2, 6));
        // y · width + x: (2, 1) → 5, (0, 1) → 3.
        assert_eq!(*buf.get(2, 1), 60);
        assert_eq!(buf[(0, 1)], 40);
        assert_eq!(buf.row(1), &[40, 50, 60]);
        *buf.get_mut(1, 0) = 99;
        buf.row_mut(1)[2] = 7;
        assert_eq!(buf.pixels(), &[10, 99, 30, 40, 50, 7]);
    }

    #[test]
    #[should_panic(expected = "pixels length must equal width * height")]
    fn new_panics_on_size_mismatch() {
        Buffer2::new(3, 2, vec![1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "do not fit in usize")]
    fn overflowing_dimensions_are_refused() {
        Buffer2::<u8>::new_default(usize::MAX, 2);
    }

    /// `(3, 0)` is in the buffer's memory — it is `(0, 1)` — but not in its first row.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "x 3 outside width 3")]
    fn a_column_past_the_width_is_refused_in_debug() {
        let buf = Buffer2::new(3, 2, vec![0u8; 6]);
        let _ = buf[(3, 0)];
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn a_row_past_the_height_is_refused() {
        let buf = Buffer2::new(3, 2, vec![0u8; 6]);
        let _ = buf.row(2);
    }

    /// RGB 2×1: R = [1, 4], G = [2, 5], B = [3, 6] ⟷ pixels [[1, 2, 3], [4, 5, 6]].
    #[test]
    fn interleave_and_deinterleave_are_inverse() {
        let planes = [
            Buffer2::new(2, 1, vec![1u8, 4]),
            Buffer2::new(2, 1, vec![2u8, 5]),
            Buffer2::new(2, 1, vec![3u8, 6]),
        ];
        let interleaved = Buffer2::interleave(planes.each_ref());
        assert_eq!(interleaved.pixels(), &[[1, 2, 3], [4, 5, 6]]);
        assert_eq!(interleaved.deinterleave(), planes);
    }

    #[test]
    #[should_panic(expected = "all channel planes must share dimensions")]
    fn interleave_rejects_mismatched_planes() {
        let planes = [
            Buffer2::new(2, 1, vec![1.0f32, 2.0]),
            Buffer2::new(1, 1, vec![3.0]),
        ];
        Buffer2::interleave(planes.each_ref());
    }
}
