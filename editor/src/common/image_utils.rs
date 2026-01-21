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
    let size = [desc.width as usize, desc.height as usize];

    let pixels: Vec<Color32> = {
        let mut bytes = image.take_bytes();
        let ptr = bytes.as_mut_ptr() as *mut Color32;
        let len = bytes.len() / 4;
        let cap = bytes.capacity() / 4;
        std::mem::forget(bytes);
        // SAFETY: Color32 is #[repr(C)] with exactly 4 bytes (RGBA),
        // and bytes are already RGBA_U8 format with proper alignment.
        unsafe { Vec::from_raw_parts(ptr, len, cap) }
    };

    ColorImage::new(size, pixels)
}
