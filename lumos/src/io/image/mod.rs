pub(crate) mod cfa;
pub(crate) mod error;
pub(crate) mod fits;
pub(crate) mod image_dimensions;
pub(crate) mod image_metadata;
pub(crate) mod image_provenance;
pub(crate) mod linear;
pub(crate) mod linear_pixels;
pub(crate) mod load_context;
pub(crate) mod preview_image;
pub(crate) mod sensor;
#[cfg(test)]
mod synthetic_tests;

use std::path::Path;

use imaginarium::{ChannelCount, ColorFormat, Image};

use crate::io::image::error::ImageError;
use crate::io::image::image_provenance::SourceContainer;

const FITS_EXTENSIONS: &[&str] = &["fits", "fit"];
const STANDARD_IMAGE_EXTENSIONS: &[&str] = &["tiff", "tif", "png", "jpg", "jpeg"];

/// Every file extension accepted by [`PreviewImage::from_file`].
pub const PREVIEW_IMAGE_EXTENSIONS: &[&str] = &[
    "fits", "fit", "raf", "cr2", "cr3", "nef", "arw", "dng", "tiff", "tif", "png", "jpg", "jpeg",
];

fn file_extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn scientific_rejection(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::ScientificInputRejected {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn read_standard_image(path: &Path) -> Result<Image, ImageError> {
    Image::read_file(path).map_err(|source| ImageError::Image {
        path: path.to_path_buf(),
        source,
    })
}

fn standard_container(extension: &str) -> SourceContainer {
    match extension {
        "tiff" | "tif" => SourceContainer::Tiff,
        "png" => SourceContainer::Png,
        "jpg" | "jpeg" => SourceContainer::Jpeg,
        _ => unreachable!("standard extension was validated before selecting its container"),
    }
}

/// The `f32` target format a given image deinterleaves into: `L_F32` for
/// grayscale, `RGB_F32` for color.
fn f32_target_format(image: &Image) -> ColorFormat {
    match image.desc().color_format.channel_count {
        ChannelCount::L => ColorFormat::L_F32,
        ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
    }
}

#[cfg(test)]
mod tests;
