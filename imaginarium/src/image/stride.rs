use aligned_vec::AVec;

use super::ALIGNMENT;

/// Aligns a value to 4-byte boundary.
pub(crate) fn align_stride(n: usize) -> usize {
    (n + 3) & !3
}

/// Adds stride padding to tightly packed pixel data.
pub(crate) fn add_stride_padding(
    src: AVec<u8>,
    width: usize,
    height: usize,
    stride: usize,
    bpp: u8,
) -> AVec<u8> {
    let row_bytes = width * bpp as usize;

    if row_bytes == stride {
        src
    } else {
        let mut padded = AVec::with_capacity(ALIGNMENT, stride * height);
        padded.resize(stride * height, 0);
        for y in 0..height {
            padded[y * stride..y * stride + row_bytes]
                .copy_from_slice(&src[y * row_bytes..y * row_bytes + row_bytes]);
        }
        padded
    }
}

/// Strips stride padding from image bytes, returning tightly packed pixel data.
pub(crate) fn strip_stride_padding(
    src: AVec<u8>,
    width: usize,
    height: usize,
    stride: usize,
    bpp: u8,
) -> AVec<u8> {
    let row_bytes = width * bpp as usize;

    if row_bytes == stride {
        src
    } else {
        let mut packed = AVec::with_capacity(ALIGNMENT, row_bytes * height);
        for y in 0..height {
            packed.extend_from_slice(&src[y * stride..y * stride + row_bytes]);
        }
        packed
    }
}
