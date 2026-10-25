//! A decoded display or inspection product, outside the scientific pipeline.

use imaginarium::InterleavedImage;

use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;

/// A decoded display or inspection product that cannot enter the scientific pipeline directly, in
/// the layout its decoder produced, so a consumer that wants planes does not get them interleaved
/// and deinterleave them again.
#[derive(Debug)]
pub enum PreviewImage {
    /// One `f32` plane per channel, with its metadata: a FITS or camera-RAW decode.
    Planes(LinearImage),
    /// Interleaved `f32` samples, and what the decoder stated of them: a raster decode.
    Interleaved {
        image: InterleavedImage,
        metadata: ImageMetadata,
    },
}

impl PreviewImage {
    /// What the decoder stated of the samples, in either layout.
    pub const fn metadata(&self) -> &ImageMetadata {
        match self {
            Self::Planes(linear) => &linear.metadata,
            Self::Interleaved { metadata, .. } => metadata,
        }
    }
}

impl From<LinearImage> for PreviewImage {
    fn from(linear: LinearImage) -> Self {
        Self::Planes(linear)
    }
}

/// The interleaved form, repacking a planar preview.
impl From<PreviewImage> for InterleavedImage {
    fn from(preview: PreviewImage) -> Self {
        match preview {
            PreviewImage::Planes(linear) => Self::from(&linear),
            PreviewImage::Interleaved { image, .. } => image,
        }
    }
}
