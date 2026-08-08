//! A decoded display or inspection product, outside the scientific pipeline.

use std::path::Path;

use imaginarium::{ChannelCount, Image};

use crate::io::image::error::ImageError;
use crate::io::image::fits::decode as fits_decode;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, TransferProvenance,
};
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::{
    FITS_EXTENSIONS, STANDARD_IMAGE_EXTENSIONS, f32_target_format, file_extension,
    read_standard_image, standard_container,
};
use crate::io::raw;

/// A decoded display or inspection product that cannot enter the scientific pipeline directly.
#[derive(Debug)]
pub struct PreviewImage {
    pub metadata: ImageMetadata,
    image: Image,
}

impl PreviewImage {
    /// Load a display or inspection image from FITS, camera RAW, TIFF, PNG, or JPEG.
    pub fn from_file<P: AsRef<Path>>(path: P, context: &LoadContext) -> Result<Self, ImageError> {
        let path = path.as_ref();
        context.check_cancelled(path)?;
        let extension = file_extension(path);

        if FITS_EXTENSIONS.contains(&extension.as_str()) {
            return fits_decode::load_preview_fits(path, context).map(Into::into);
        }

        if raw::RAW_EXTENSIONS.contains(&extension.as_str()) {
            return raw::load_raw(path, &context.cancel).map(Into::into);
        }

        if STANDARD_IMAGE_EXTENSIONS.contains(&extension.as_str()) {
            let decoded = read_standard_image(path)?;
            context.check_cancelled(path)?;
            let alpha_dropped = decoded.desc().color_format.channel_count == ChannelCount::Rgba;
            let target = f32_target_format(&decoded);
            let image = decoded
                .convert(target)
                .expect("standard image converts to its f32 channel format");
            let metadata = ImageMetadata {
                provenance: Some(ImageProvenance {
                    container: standard_container(&extension),
                    decoder: DecoderProvenance::Imaginarium,
                    transfer: TransferProvenance::UnspecifiedRaster,
                    color: ColorProvenance::UnmanagedRaster { alpha_dropped },
                    clipped: false,
                    demosaic: DemosaicProvenance::None,
                }),
                ..Default::default()
            };
            return Ok(Self { metadata, image });
        }

        Err(ImageError::UnsupportedFormat { extension })
    }
}

impl From<LinearImage> for PreviewImage {
    fn from(linear: LinearImage) -> Self {
        let image = Image::from(&linear);
        Self {
            metadata: linear.metadata,
            image,
        }
    }
}

impl From<PreviewImage> for Image {
    fn from(preview: PreviewImage) -> Self {
        preview.image
    }
}
