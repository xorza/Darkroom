use egui::Color32;
use egui::epaint::ColorImage;
use imaginarium::{ColorFormat, Image};

/// Converts an imaginarium Image to an egui ColorImage.
/// The image must be in RGBA_U8 format.
pub fn to_color_image(image: Image) -> ColorImage {
    assert_eq!(
        image.desc().color_format,
        ColorFormat::RGBA_U8,
        "Image must be RGBA_U8 format"
    );

    let desc = *image.desc();
    let size = [desc.width, desc.height];

    // Copy rather than reinterpret: `Vec<u8>` is align-1, `Color32` is
    // align-4 (`#[repr(align(4))]`), so `Vec::from_raw_parts` across the
    // two would mismatch `alloc`/`dealloc` layouts. We allocate a
    // properly-aligned destination and `memcpy` the bytes in — `Color32`'s
    // `[r,g,b,a]` byte layout matches RGBA_U8 verbatim.
    let bytes = image.into_bytes();
    assert_eq!(
        bytes.len() % 4,
        0,
        "RGBA_U8 buffer length must be 4-aligned"
    );
    let pixel_count = bytes.len() / 4;
    let mut pixels: Vec<Color32> = Vec::with_capacity(pixel_count);
    // SAFETY: dst is align-4 (from `Vec::with_capacity::<Color32>`),
    // capacity is `pixel_count` Color32s = `bytes.len()` bytes, src is
    // a distinct allocation, and Color32's layout (`#[repr(C, align(4))]`
    // wrapping `[u8; 4]`) is byte-identical to RGBA_U8.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pixels.as_mut_ptr() as *mut u8, bytes.len());
        pixels.set_len(pixel_count);
    }

    ColorImage::new(size, pixels)
}
