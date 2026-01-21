/// Aligns a value to 4-byte boundary.
pub(crate) fn align_stride(n: usize) -> usize {
    (n + 3) & !3
}

/// Adds stride padding to tightly packed pixel data.
pub(crate) fn add_stride_padding(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bpp: u8,
) -> Vec<u8> {
    let row_bytes = width as usize * bpp as usize;

    if row_bytes == stride {
        src.to_vec()
    } else {
        let mut padded = vec![0u8; stride * height as usize];
        for y in 0..height as usize {
            padded[y * stride..y * stride + row_bytes]
                .copy_from_slice(&src[y * row_bytes..y * row_bytes + row_bytes]);
        }
        padded
    }
}

/// Strips stride padding from image bytes, returning tightly packed pixel data.
pub(crate) fn strip_stride_padding(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bpp: u8,
) -> Vec<u8> {
    let row_bytes = width as usize * bpp as usize;

    if row_bytes == stride {
        src.to_vec()
    } else {
        let mut packed = Vec::with_capacity(row_bytes * height as usize);
        for y in 0..height as usize {
            packed.extend_from_slice(&src[y * stride..y * stride + row_bytes]);
        }
        packed
    }
}
