//! Loading the non-FITS image formats, and the file-extension tables that route to them.
//!
//! FITS and RAW have their own decoders under `fits/` and `io/raw/`; what is left — TIFF, PNG,
//! JPEG — is read through imaginarium in one call, so it needs only these few helpers rather than
//! a module of its own machinery.

use std::path::Path;

use imaginarium::{ChannelCount, ColorFormat, Image};

use crate::io::image::error::ImageError;
use crate::io::image::image_provenance::SourceContainer;

pub(crate) const FITS_EXTENSIONS: &[&str] = &["fits", "fit"];
pub(crate) const STANDARD_IMAGE_EXTENSIONS: &[&str] = &["tiff", "tif", "png", "jpg", "jpeg"];

pub(crate) fn file_extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub(crate) fn scientific_rejection(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::ScientificInputRejected {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

pub(crate) fn read_standard_image(path: &Path) -> Result<Image, ImageError> {
    Image::read_file(path).map_err(|source| ImageError::Image {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn standard_container(extension: &str) -> SourceContainer {
    match extension {
        "tiff" | "tif" => SourceContainer::Tiff,
        "png" => SourceContainer::Png,
        "jpg" | "jpeg" => SourceContainer::Jpeg,
        _ => unreachable!("standard extension was validated before selecting its container"),
    }
}

/// The `f32` target format a given image deinterleaves into: `L_F32` for
/// grayscale, `RGB_F32` for color.
pub(crate) fn f32_target_format(image: &Image) -> ColorFormat {
    match image.desc().color_format.channel_count {
        ChannelCount::L => ColorFormat::L_F32,
        ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
    }
}
