//! Statically typed interleaved pixels: one `[T; N]` value per image pixel.

use crate::common::buffer2::Buffer2;

/// Tightly packed `width × height` pixels with `N` interleaved channels.
#[derive(Debug, Clone)]
pub(crate) struct InterleavedPixels<const N: usize, T> {
    pub(crate) buffer: Buffer2<[T; N]>,
}

impl<const N: usize, T: bytemuck::Pod> InterleavedPixels<N, T> {
    pub(crate) fn from_buffer(buffer: Buffer2<[T; N]>) -> Self {
        const {
            assert!(
                N == 1 || N == 3 || N == 4,
                "InterleavedPixels supports 1, 3, or 4 channels"
            )
        };
        Self { buffer }
    }

    pub(crate) fn new_zeroed(width: usize, height: usize) -> Self {
        Self::from_buffer(Buffer2::new_filled(width, height, [T::zeroed(); N]))
    }

    pub(crate) fn from_bytes(width: usize, height: usize, bytes: &[u8]) -> Self {
        let pixel_count = width
            .checked_mul(height)
            .expect("image pixel count must fit in usize");
        let byte_count = pixel_count
            .checked_mul(std::mem::size_of::<[T; N]>())
            .expect("image byte count must fit in usize");
        assert_eq!(
            bytes.len(),
            byte_count,
            "byte length must equal width * height * size_of::<[T; N]>()"
        );

        let mut pixels = vec![[T::zeroed(); N]; pixel_count];
        bytemuck::cast_slice_mut::<T, u8>(pixels.as_flattened_mut()).copy_from_slice(bytes);
        Self::from_buffer(Buffer2::new(width, height, pixels))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.buffer.pixels().as_flattened())
    }

    pub(crate) fn as_bytes_mut(&mut self) -> &mut [u8] {
        bytemuck::cast_slice_mut(self.buffer.pixels_mut().as_flattened_mut())
    }
}
