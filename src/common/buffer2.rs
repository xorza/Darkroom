//! `Buffer2<T>` — a generic 2D buffer over `Vec<T>` with `(x, y)`, linear, and
//! range indexing plus `Deref` to `[T]`. Imaginarium owns the type and stores
//! its size as plain `usize` fields. Both pixel layouts build on it: interleaved
//! `InterleavedPixels` stores `Buffer2<[T; N]>`, while `PlanarPixels` stores one
//! `Buffer2<T>` per channel. Lumos uses it for `LinearImage` channel planes.

use std::ops::{Deref, DerefMut, Index, IndexMut, Range};
use std::slice;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Buffer2<T> {
    pixels: Vec<T>,
    width: usize,
    height: usize,
}

impl<T> Buffer2<T> {
    pub fn new(width: usize, height: usize, pixels: Vec<T>) -> Self {
        assert_eq!(
            pixels.len(),
            width * height,
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
        assert!(x < self.width && y < self.height);
        &self.pixels[y * self.width + x]
    }

    #[inline]
    pub fn get_mut(&mut self, x: usize, y: usize) -> &mut T {
        assert!(x < self.width && y < self.height);
        &mut self.pixels[y * self.width + x]
    }

    #[inline]
    pub fn row(&self, y: usize) -> &[T] {
        assert!(y < self.height);
        let start = y * self.width;
        &self.pixels[start..start + self.width]
    }

    #[inline]
    pub fn row_mut(&mut self, y: usize) -> &mut [T] {
        assert!(y < self.height);
        let start = y * self.width;
        &mut self.pixels[start..start + self.width]
    }

    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }

    #[inline]
    pub fn height(&self) -> usize {
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

    #[inline]
    pub fn copy_from(&mut self, other: &Self)
    where
        T: Copy,
    {
        assert_eq!(self.width, other.width, "width mismatch");
        assert_eq!(self.height, other.height, "height mismatch");
        self.pixels.copy_from_slice(&other.pixels);
    }
}

impl<T: Default + Clone> Buffer2<T> {
    pub fn new_default(width: usize, height: usize) -> Self {
        Self {
            pixels: vec![T::default(); width * height],
            width,
            height,
        }
    }
}

impl<T: Clone> Buffer2<T> {
    pub fn new_filled(width: usize, height: usize, value: T) -> Self {
        Self {
            pixels: vec![value; width * height],
            width,
            height,
        }
    }
}

impl<T> Index<(usize, usize)> for Buffer2<T> {
    type Output = T;

    #[inline]
    fn index(&self, (x, y): (usize, usize)) -> &Self::Output {
        &self.pixels[y * self.width + x]
    }
}

impl<T> IndexMut<(usize, usize)> for Buffer2<T> {
    #[inline]
    fn index_mut(&mut self, (x, y): (usize, usize)) -> &mut Self::Output {
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
    type IntoIter = std::vec::IntoIter<T>;

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
    use super::*;

    #[test]
    fn new_stores_dimensions_and_pixels() {
        let buf = Buffer2::new(3, 2, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!(buf.width(), 3);
        assert_eq!(buf.height(), 2);
        assert_eq!(buf.len(), 6); // via Deref to [T]
        assert_eq!(*buf.get(2, 1), 60); // y*width + x = 1*3+2 = 5
        assert_eq!(buf[(0, 1)], 40); // 1*3+0 = 3
    }

    #[test]
    #[should_panic(expected = "pixels length must equal width * height")]
    fn new_panics_on_size_mismatch() {
        Buffer2::new(3, 2, vec![1, 2, 3]);
    }

    #[test]
    fn new_default_is_zeroed() {
        let buf: Buffer2<f32> = Buffer2::new_default(4, 3);
        assert_eq!(buf.len(), 12);
        assert!(buf.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn get_mut_and_copy_from() {
        let mut buf = Buffer2::new(2, 2, vec![1, 2, 3, 4]);
        *buf.get_mut(1, 0) = 99;
        assert_eq!(*buf.get(1, 0), 99);

        let src = Buffer2::new(2, 2, vec![10, 20, 30, 40]);
        let mut dst = Buffer2::new_default(2, 2);
        dst.copy_from(&src);
        assert_eq!(dst.pixels(), src.pixels());
    }
}
