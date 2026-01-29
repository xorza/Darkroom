use std::ops::{Deref, DerefMut, Index, IndexMut, Range};
use std::slice;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn as_ptr(&self) -> *const T {
        self.pixels.as_ptr()
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> &T {
        debug_assert!(x < self.width && y < self.height);
        &self.pixels[y * self.width + x]
    }

    #[inline]
    pub fn get_mut(&mut self, x: usize, y: usize) -> &mut T {
        debug_assert!(x < self.width && y < self.height);
        &mut self.pixels[y * self.width + x]
    }

    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.pixels.len()
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
    pub fn to_vec(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.pixels.clone()
    }

    #[inline]
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.pixels.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, T> {
        self.pixels.iter_mut()
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

    #[inline]
    pub fn fill(&mut self, value: T) {
        self.pixels.fill(value);
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

impl<T> AsRef<[T]> for Buffer2<T> {
    #[inline]
    fn as_ref(&self) -> &[T] {
        &self.pixels
    }
}

impl<T> AsMut<[T]> for Buffer2<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut [T] {
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
