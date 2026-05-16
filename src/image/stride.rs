use aligned_vec::{AVec, ConstAlign};

use super::ALIGNMENT;

/// Aligns a value to 4-byte boundary.
pub(crate) fn align_stride(n: usize) -> usize {
    (n + 3) & !3
}

/// Adds stride padding to tightly packed pixel data.
pub(crate) fn add_stride_padding(
    src: AVec<u8, ConstAlign<ALIGNMENT>>,
    width: usize,
    height: usize,
    stride: usize,
    bpp: u8,
) -> AVec<u8, ConstAlign<ALIGNMENT>> {
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

/// Strips stride padding from a byte slice, returning tightly packed pixel data.
/// Returns None if already packed (stride == row_bytes).
pub(crate) fn strip_stride_padding_from_slice(
    src: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    bpp: u8,
) -> Option<AVec<u8, ConstAlign<ALIGNMENT>>> {
    let row_bytes = width * bpp as usize;

    if row_bytes == stride {
        None
    } else {
        let mut packed = AVec::with_capacity(ALIGNMENT, row_bytes * height);
        for y in 0..height {
            packed.extend_from_slice(&src[y * stride..y * stride + row_bytes]);
        }
        Some(packed)
    }
}
