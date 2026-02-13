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
    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_stores_dimensions() {
        let buf = Buffer2::new(3, 2, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!(buf.width(), 3);
        assert_eq!(buf.height(), 2);
        assert_eq!(buf.len(), 6);
        assert!(!buf.is_empty());
    }

    #[test]
    #[should_panic(expected = "pixels length must equal width * height")]
    fn test_new_panics_on_size_mismatch() {
        Buffer2::new(3, 2, vec![1, 2, 3]); // 3 != 3*2
    }

    #[test]
    fn test_new_default() {
        let buf: Buffer2<f32> = Buffer2::new_default(4, 3);
        assert_eq!(buf.width(), 4);
        assert_eq!(buf.height(), 3);
        assert_eq!(buf.len(), 12);
        assert!(buf.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_new_filled() {
        let buf = Buffer2::new_filled(2, 3, 42u8);
        assert_eq!(buf.len(), 6);
        assert!(buf.iter().all(|&v| v == 42));
    }

    #[test]
    fn test_get_2d() {
        // 3x2 buffer: row 0 = [10, 20, 30], row 1 = [40, 50, 60]
        let buf = Buffer2::new(3, 2, vec![10, 20, 30, 40, 50, 60]);
        // get(x, y) => pixels[y * width + x]
        assert_eq!(*buf.get(0, 0), 10); // (0,0) => [0*3+0=0]
        assert_eq!(*buf.get(2, 0), 30); // (2,0) => [0*3+2=2]
        assert_eq!(*buf.get(0, 1), 40); // (0,1) => [1*3+0=3]
        assert_eq!(*buf.get(2, 1), 60); // (2,1) => [1*3+2=5]
    }

    #[test]
    fn test_get_mut_2d() {
        let mut buf = Buffer2::new(2, 2, vec![1, 2, 3, 4]);
        *buf.get_mut(1, 0) = 99;
        assert_eq!(*buf.get(1, 0), 99);
        assert_eq!(*buf.get(0, 0), 1); // unchanged
    }

    #[test]
    fn test_index_tuple() {
        let buf = Buffer2::new(3, 2, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!(buf[(0, 0)], 10);
        assert_eq!(buf[(2, 1)], 60); // (2,1) => 1*3+2 = 5
    }

    #[test]
    fn test_index_mut_tuple() {
        let mut buf = Buffer2::new(2, 2, vec![1, 2, 3, 4]);
        buf[(1, 1)] = 77; // (1,1) => index 3
        assert_eq!(buf[(1, 1)], 77);
    }

    #[test]
    fn test_index_linear() {
        let buf = Buffer2::new(3, 2, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!(buf[0], 10);
        assert_eq!(buf[5], 60);
    }

    #[test]
    fn test_index_range() {
        let buf = Buffer2::new(3, 2, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!(&buf[1..4], &[20, 30, 40]);
    }

    #[test]
    fn test_into_vec() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0];
        let buf = Buffer2::new(2, 2, data.clone());
        assert_eq!(buf.into_vec(), data);
    }

    #[test]
    fn test_from_buffer2_to_vec() {
        let buf = Buffer2::new(2, 2, vec![1, 2, 3, 4]);
        let v: Vec<i32> = buf.into();
        assert_eq!(v, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_copy_from() {
        let src = Buffer2::new(2, 2, vec![10, 20, 30, 40]);
        let mut dst = Buffer2::new(2, 2, vec![0, 0, 0, 0]);
        dst.copy_from(&src);
        assert_eq!(dst.pixels(), src.pixels());
    }

    #[test]
    #[should_panic(expected = "width mismatch")]
    fn test_copy_from_panics_on_width_mismatch() {
        let src = Buffer2::new(3, 2, vec![0; 6]);
        let mut dst = Buffer2::new(2, 3, vec![0; 6]);
        dst.copy_from(&src);
    }

    #[test]
    fn test_fill() {
        let mut buf = Buffer2::new(2, 2, vec![1, 2, 3, 4]);
        buf.fill(99);
        assert!(buf.iter().all(|&v| v == 99));
    }

    #[test]
    fn test_deref_to_slice() {
        let buf = Buffer2::new(2, 2, vec![1, 2, 3, 4]);
        let slice: &[i32] = &buf;
        assert_eq!(slice.len(), 4);
        assert_eq!(slice[0], 1);
    }

    #[test]
    fn test_index_method() {
        let buf = Buffer2::<u8>::new_default(5, 3);
        // index(x, y) = y * width + x
        assert_eq!(buf.index(0, 0), 0);
        assert_eq!(buf.index(4, 0), 4);
        assert_eq!(buf.index(0, 1), 5); // 1 * 5 + 0
        assert_eq!(buf.index(3, 2), 13); // 2 * 5 + 3
    }

    #[test]
    fn test_iterator_sum() {
        let buf = Buffer2::new(2, 2, vec![10, 20, 30, 40]);
        let sum: i32 = buf.iter().sum();
        assert_eq!(sum, 100); // 10 + 20 + 30 + 40
    }

    #[test]
    fn test_clone_equality() {
        let buf = Buffer2::new(2, 2, vec![1, 2, 3, 4]);
        let cloned = buf.clone();
        assert_eq!(buf, cloned);
    }
}
