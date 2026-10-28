//! `Buffer2<T>` — a generic 2D buffer over `Vec<T>` with `(x, y)`, linear, and
//! range indexing plus `Deref` to `[T]`. An interleaved image stores
//! `Buffer2<[T; N]>`; a planar one, one `Buffer2<T>` per channel, and
//! [`Buffer2::interleave`] / [`Buffer2::deinterleave`] move between the two.

use std::ops::{Deref, DerefMut, Index, IndexMut, Range};
use std::{array, slice, vec};

use glam::USizeVec2;
use rayon::iter::{IndexedParallelIterator, ParallelIterator};

use crate::par;
use crate::usize2::USize2;

/// `width × height` values in row-major order.
///
/// Every indexed access is bounds-checked in release against the whole buffer;
/// that `x < width` — which keeps a column from spilling into the next row — is
/// checked in debug builds, where a per-pixel accessor can afford it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer2<T> {
    pixels: Vec<T>,
    size: USize2,
}

impl<T> Buffer2<T> {
    pub fn new(size: impl Into<USize2>, pixels: Vec<T>) -> Self {
        let size = size.into();
        assert_eq!(
            pixels.len(),
            size.pixel_count(),
            "pixels length must equal width * height"
        );
        Self { pixels, size }
    }

    /// The buffer of `size` whose value at each pixel is `f` of its position, called once per
    /// pixel in row-major order — so a stateful `f`, a random draw, lays its values out row by row.
    pub fn from_fn(size: impl Into<USize2>, mut f: impl FnMut(USizeVec2) -> T) -> Self {
        let size = size.into();
        let mut pixels = Vec::with_capacity(size.pixel_count());
        for y in 0..size.height {
            pixels.extend((0..size.width).map(|x| f(USizeVec2::new(x, y))));
        }
        Self { pixels, size }
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
        &self.pixels[y * self.size.width..][..self.size.width]
    }

    #[inline]
    pub fn row_mut(&mut self, y: usize) -> &mut [T] {
        &mut self.pixels[y * self.size.width..][..self.size.width]
    }

    #[inline]
    pub const fn size(&self) -> USize2 {
        self.size
    }

    #[inline]
    pub const fn width(&self) -> usize {
        self.size.width
    }

    #[inline]
    pub const fn height(&self) -> usize {
        self.size.height
    }

    #[inline]
    pub fn pixels(&self) -> &[T] {
        &self.pixels
    }

    #[inline]
    pub fn pixels_mut(&mut self) -> &mut [T] {
        &mut self.pixels
    }

    /// The values its allocation holds, which [`reshape`](Self::reshape) grows only for a larger
    /// size.
    #[inline]
    pub const fn capacity(&self) -> usize {
        self.pixels.capacity()
    }

    #[inline]
    pub fn into_vec(self) -> Vec<T> {
        self.pixels
    }
}

/// The empty 0×0 buffer, with no allocation: a scratch plane before its first
/// [`reshape`](Buffer2::reshape).
impl<T> Default for Buffer2<T> {
    fn default() -> Self {
        Self {
            pixels: Vec::new(),
            size: USize2::default(),
        }
    }
}

impl<T: Default + Clone> Buffer2<T> {
    pub fn new_default(size: impl Into<USize2>) -> Self {
        Self::new_filled(size, T::default())
    }
}

impl<T: Clone> Buffer2<T> {
    pub fn new_filled(size: impl Into<USize2>, value: T) -> Self {
        let size = size.into();
        Self {
            pixels: vec![value; size.pixel_count()],
            size,
        }
    }

    /// This buffer as `size`, in the allocation it has: the first `width · height` of its values
    /// stay where they lie in row-major order, and any past the old length are `value`. For a
    /// scratch plane reused across sizes, which grows only for a larger one.
    pub fn reshape(&mut self, size: impl Into<USize2>, value: T) {
        self.size = size.into();
        self.pixels.resize(self.size.pixel_count(), value);
    }
}

impl<T: Copy + Send + Sync, const N: usize> Buffer2<[T; N]> {
    /// Interleaves `N` channel planes into one pixel buffer.
    ///
    /// # Panics
    /// Unless every plane has the size of the first.
    pub fn interleave(planes: [&Buffer2<T>; N]) -> Self {
        let size = planes[0].size;
        for plane in planes {
            assert_eq!(plane.size, size, "all channel planes must share dimensions");
        }
        let pixels = par::into_iter(0..size.pixel_count())
            .map(|i| planes.map(|plane| plane.pixels[i]))
            .collect();
        Self::new(size, pixels)
    }

    /// The `N` channel planes of this pixel buffer.
    pub fn deinterleave(&self) -> [Buffer2<T>; N]
    where
        T: Default,
    {
        self.channel_planes(|sample| sample)
    }

    /// The first `C` channels of this pixel buffer as planes, each sample through `sample`: one
    /// row-parallel pass that reads each pixel once and writes every plane.
    pub fn channel_planes<U: Copy + Default + Send, const C: usize>(
        &self,
        sample: impl Fn(T) -> U + Sync,
    ) -> [Buffer2<U>; C] {
        const { assert!(C <= N, "a pixel's first channels") };
        if self.pixels.is_empty() {
            return array::from_fn(|_| Buffer2::new(self.size, Vec::new()));
        }
        let width = self.size.width;
        let mut planes: [Vec<U>; C] = array::from_fn(|_| vec![U::default(); self.pixels.len()]);
        let mut chunks = planes.each_mut().map(|plane| plane.chunks_mut(width));
        let rows: Vec<[&mut [U]; C]> = (0..self.size.height)
            .map(|_| {
                chunks
                    .each_mut()
                    .map(|rows| rows.next().expect("a row of each plane"))
            })
            .collect();
        par::into_iter(rows)
            .zip(par::chunks(&self.pixels, width))
            .for_each(|(mut out, pixels)| {
                for (x, pixel) in pixels.iter().enumerate() {
                    for (plane, &value) in out.iter_mut().zip(pixel) {
                        plane[x] = sample(value);
                    }
                }
            });
        planes.map(|plane| Buffer2::new(self.size, plane))
    }

    /// [`Self::deinterleave`], taking the buffer: one channel keeps its allocation, as its pixels
    /// already are its plane.
    pub fn into_planes(self) -> [Buffer2<T>; N]
    where
        T: Default,
    {
        if N > 1 {
            return self.deinterleave();
        }
        let mut plane = Some(Buffer2::new(self.size, self.pixels.into_flattened()));
        array::from_fn(|_| plane.take().expect("one channel"))
    }
}

impl<T> Index<USizeVec2> for Buffer2<T> {
    type Output = T;

    #[inline]
    fn index(&self, pos: USizeVec2) -> &Self::Output {
        &self[(pos.x, pos.y)]
    }
}

impl<T> IndexMut<USizeVec2> for Buffer2<T> {
    #[inline]
    fn index_mut(&mut self, pos: USizeVec2) -> &mut Self::Output {
        &mut self[(pos.x, pos.y)]
    }
}

impl<T> Index<(usize, usize)> for Buffer2<T> {
    type Output = T;

    #[inline]
    fn index(&self, (x, y): (usize, usize)) -> &Self::Output {
        debug_assert!(
            x < self.size.width,
            "x {x} outside width {}",
            self.size.width
        );
        &self.pixels[y * self.size.width + x]
    }
}

impl<T> IndexMut<(usize, usize)> for Buffer2<T> {
    #[inline]
    fn index_mut(&mut self, (x, y): (usize, usize)) -> &mut Self::Output {
        debug_assert!(
            x < self.size.width,
            "x {x} outside width {}",
            self.size.width
        );
        &mut self.pixels[y * self.size.width + x]
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
    use crate::buffer2::Buffer2;
    use crate::usize2::USize2;
    use glam::USizeVec2;

    #[test]
    fn indexing_is_row_major() {
        let mut buf = Buffer2::new((3, 2), vec![10, 20, 30, 40, 50, 60]);
        assert_eq!(buf.size(), USize2::new(3, 2));
        assert_eq!((buf.width(), buf.height(), buf.len()), (3, 2, 6));
        // y · width + x: (2, 1) → 5, (0, 1) → 3.
        assert_eq!(*buf.get(2, 1), 60);
        assert_eq!(buf[(0, 1)], 40);
        assert_eq!(buf[USizeVec2::new(0, 1)], 40);
        assert_eq!(buf.row(1), &[40, 50, 60]);
        *buf.get_mut(1, 0) = 99;
        buf.row_mut(1)[2] = 7;
        assert_eq!(buf.pixels(), &[10, 99, 30, 40, 50, 7]);
    }

    #[test]
    #[should_panic(expected = "pixels length must equal width * height")]
    fn new_panics_on_size_mismatch() {
        Buffer2::new((3, 2), vec![1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "grid pixel count must fit in usize")]
    fn overflowing_dimensions_are_refused() {
        Buffer2::<u8>::new_default((usize::MAX, 2));
    }

    /// `(3, 0)` is in the buffer's memory — it is `(0, 1)` — but not in its first row.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "x 3 outside width 3")]
    fn a_column_past_the_width_is_refused_in_debug() {
        let buf = Buffer2::new((3, 2), vec![0u8; 6]);
        let _ = buf[(3, 0)];
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn a_row_past_the_height_is_refused() {
        let buf = Buffer2::new((3, 2), vec![0u8; 6]);
        let _ = buf.row(2);
    }

    /// `f` runs once per pixel in row-major order with each pixel's own `(x, y)`: 3×2 of `10·y + x`
    /// is `[0, 1, 2, 10, 11, 12]`, and a counter it advances numbers the pixels 0 to 5 in that
    /// order. An empty side calls it never.
    #[test]
    fn from_fn_takes_each_position_in_row_major_order() {
        let positions = Buffer2::from_fn((3, 2), |at| 10 * at.y + at.x);
        assert_eq!((positions.width(), positions.height()), (3, 2));
        assert_eq!(positions.pixels(), [0, 1, 2, 10, 11, 12]);
        assert_eq!(*positions.get(2, 1), 12);
        let mut next = 0;
        let order = Buffer2::from_fn((3, 2), |_| {
            next += 1;
            next - 1
        });
        assert_eq!(order.pixels(), [0, 1, 2, 3, 4, 5]);
        let mut calls = 0;
        let empty = Buffer2::<u8>::from_fn((0, 4), |_| {
            calls += 1;
            0
        });
        assert_eq!((empty.width(), empty.height(), calls), (0, 4, 0));
    }

    /// The default buffer is 0×0. Shrinking keeps the first values in their order and the
    /// allocation; growing keeps them all and fills the rest: 2×2 `[1, 2, 3, 4]` is `[1, 2, 3]` as
    /// 3×1, then `[1, 2, 3, 9, 9, 9]` as 2×3.
    #[test]
    fn reshape_keeps_the_values_in_order_and_the_allocation() {
        let empty = Buffer2::<u8>::default();
        assert_eq!(
            (empty.width(), empty.height(), empty.pixels()),
            (0, 0, &[][..])
        );
        let mut buffer = Buffer2::new((2, 2), vec![1, 2, 3, 4]);
        let start = buffer.pixels().as_ptr();
        buffer.reshape((3, 1), 0);
        assert_eq!((buffer.width(), buffer.height()), (3, 1));
        assert_eq!(buffer.pixels(), [1, 2, 3]);
        assert_eq!(buffer.pixels().as_ptr(), start);
        assert_eq!(buffer.capacity(), 4);
        assert_eq!(buffer.row(0), [1, 2, 3]);
        buffer.reshape((2, 3), 9);
        assert_eq!((buffer.width(), buffer.height()), (2, 3));
        assert_eq!(buffer.pixels(), [1, 2, 3, 9, 9, 9]);
        assert_eq!(buffer.row(2), [9, 9]);
    }

    /// RGB 2×1: R = [1, 4], G = [2, 5], B = [3, 6] ⟷ pixels [[1, 2, 3], [4, 5, 6]], borrowed or
    /// taken; the first two channels, each times ten, are [10, 40] and [20, 50]; a one-channel
    /// buffer taken is its plane, in the same allocation.
    #[test]
    fn interleave_and_deinterleave_are_inverse() {
        let planes = [
            Buffer2::new((2, 1), vec![1u8, 4]),
            Buffer2::new((2, 1), vec![2u8, 5]),
            Buffer2::new((2, 1), vec![3u8, 6]),
        ];
        let interleaved = Buffer2::interleave(planes.each_ref());
        assert_eq!(interleaved.pixels(), &[[1, 2, 3], [4, 5, 6]]);
        assert_eq!(interleaved.deinterleave(), planes);
        let [red, green] = interleaved.channel_planes(|sample| u16::from(sample) * 10);
        assert_eq!([red.pixels(), green.pixels()], [&[10, 40], &[20, 50]]);
        assert_eq!(interleaved.into_planes(), planes);

        let grey = Buffer2::new((3, 1), vec![[7.0f32], [8.0], [9.0]]);
        let pointer = grey.pixels().as_ptr().cast::<f32>();
        let [plane] = grey.into_planes();
        assert_eq!(plane, Buffer2::new((3, 1), vec![7.0, 8.0, 9.0]));
        assert_eq!(plane.pixels().as_ptr(), pointer);
    }

    #[test]
    #[should_panic(expected = "all channel planes must share dimensions")]
    fn interleave_rejects_mismatched_planes() {
        let planes = [
            Buffer2::new((2, 1), vec![1.0f32, 2.0]),
            Buffer2::new((1, 1), vec![3.0]),
        ];
        Buffer2::interleave(planes.each_ref());
    }
}
