use std::ops::{Index, IndexMut};

#[derive(Debug, Clone)]
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
    pub fn into_pixels(self) -> Vec<T> {
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
