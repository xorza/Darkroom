//! Loading the non-FITS image formats.
//!
//! FITS and RAW have their own decoders under `fits/` and `io/raw/`; what is left — TIFF, PNG,
//! JPEG — is read through imaginarium in one call, so it needs only these few helpers rather than
//! a module of its own machinery.

use std::path::Path;

use imaginarium::{ChannelCount, ColorFormat, Image};

use crate::io::image::error::ImageError;

pub(crate) fn read_standard_image(path: &Path) -> Result<Image, ImageError> {
    Image::read_file(path).map_err(|source| ImageError::Image {
        path: path.to_path_buf(),
        source,
    })
}

/// The `f32` target format a given image deinterleaves into: `L_F32` for
/// grayscale, `RGB_F32` for color.
pub(crate) const fn f32_target_format(image: &Image) -> ColorFormat {
    match image.desc().color_format.channel_count {
        ChannelCount::L => ColorFormat::L_F32,
        ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
    }
}
