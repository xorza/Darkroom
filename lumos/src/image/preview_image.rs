//! A decoded display or inspection product, outside the scientific pipeline.

use std::mem;

use imaginarium::Image;

use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;

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
    /// A raster decode, `f32` samples interleaved as it read them, with `metadata`.
    pub(crate) const fn interleaved(image: Image, metadata: ImageMetadata) -> Self {
        Self {
            metadata,
            pixels: PreviewPixels::Interleaved(image),
        }
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
            PreviewPixels::Planes(linear) => Self::from(&linear),
            PreviewPixels::Interleaved(image) => image,
        }
    }
}
