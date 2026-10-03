//! A decoded display or inspection product, outside the scientific pipeline.

use std::mem;
use std::path::Path;

use imaginarium::{ChannelCount, Image};

use crate::io::image::error::ImageError;
use crate::io::image::fits::decode as fits_decode;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
use crate::io::image::input_format::InputFormat;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::standard::{f32_target_format, read_standard_image};
use crate::io::raw;

/// A decoded display or inspection product that cannot enter the scientific pipeline directly.
#[derive(Debug)]
pub struct PreviewImage {
    pub metadata: ImageMetadata,
    pixels: PreviewPixels,
}

/// A preview's pixels in the layout its decoder produced, so a consumer that
/// wants planes does not get them interleaved and deinterleave them again.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "one per load, beside megabytes of pixels; boxing the planes would buy an indirection"
)]
pub enum PreviewPixels {
    /// One `f32` plane per channel: a FITS or camera-RAW decode.
    Planes(LinearImage),
    /// Interleaved `f32` samples: a raster decode.
    Interleaved(Image),
}

impl PreviewImage {
    /// Load a display or inspection image from FITS, camera RAW, TIFF, PNG, or JPEG.
    pub fn from_file<P: AsRef<Path>>(path: P, context: &LoadContext) -> Result<Self, ImageError> {
        let path = path.as_ref();
        context.check_cancelled(path)?;
        let format = match InputFormat::of(path)? {
            InputFormat::Fits => {
                return fits_decode::load_preview_fits(path, context).map(Into::into);
            }
            InputFormat::CameraRaw => return raw::load_raw(path, context).map(Into::into),
            InputFormat::Raster(format) => format,
        };
        let decoded = read_standard_image(path)?;
        context.check_cancelled(path)?;
        let alpha_dropped = decoded.desc().color_format.channel_count == ChannelCount::Rgba;
        let target = f32_target_format(&decoded);
        let image = decoded.convert(target);
        let metadata = ImageMetadata {
            provenance: Some(ImageProvenance {
                container: SourceContainer::from(format),
                decoder: DecoderProvenance::Imaginarium,
                transfer: TransferProvenance::UnspecifiedRaster,
                color: ColorProvenance::UnmanagedRaster { alpha_dropped },
                clipped: false,
                demosaic: DemosaicProvenance::None,
                // Every raster format this path reads stores its first row at the top.
                row_order: RowOrder::TopDown,
            }),
            ..Default::default()
        };
        Ok(Self {
            metadata,
            pixels: PreviewPixels::Interleaved(image),
        })
    }

    /// The pixels, in the layout the decoder produced.
    pub fn into_pixels(self) -> PreviewPixels {
        self.pixels
    }
}

impl From<LinearImage> for PreviewImage {
    fn from(mut linear: LinearImage) -> Self {
        Self {
            metadata: mem::take(&mut linear.metadata),
            pixels: PreviewPixels::Planes(linear),
        }
    }
}

/// The interleaved form, repacking a planar preview.
impl From<PreviewImage> for Image {
    fn from(preview: PreviewImage) -> Self {
        match preview.pixels {
            PreviewPixels::Planes(linear) => Image::from(&linear),
            PreviewPixels::Interleaved(image) => image,
        }
    }
}
