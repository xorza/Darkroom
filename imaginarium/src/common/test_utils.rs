use crate::image::Image;
use crate::prelude::*;

/// Loads the lena test image as RGBA_U8 format.
pub fn load_lena_rgba_u8() -> Image {
    Image::read_file("test_resources/lena.tiff")
        .unwrap()
        .convert(ColorFormat::from((
            ChannelCount::Rgba,
            ChannelSize::_8bit,
            ChannelType::UInt,
        )))
        .unwrap()
}
