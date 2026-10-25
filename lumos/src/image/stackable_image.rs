//! [`StackableImage`]: an image as the frame layer reads it, whatever its type.

use std::fmt::Debug;

use arrayvec::ArrayVec;
use common::Buffer2;

use crate::image::cfa::cfa_type::CfaType;
use crate::image::flag_counts::FlagCounts;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::pixel_flags::PixelFlags;

/// An image as the frame layer reads it: its planes, flags, metadata and pattern, whatever its
/// type. Reading one from a file is a [`Decoder`](crate::io::decoder::Decoder)'s.
pub(crate) trait StackableImage: Send + Sync + Debug + Sized {
    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, channel: usize) -> &Buffer2<f32>;
    fn metadata(&self) -> &ImageMetadata;
    /// The sensor pattern of an undemosaiced sensor frame; `None` for any other image.
    fn cfa_type(&self) -> Option<CfaType>;

    /// The image's data-quality flags, for an image that carries any.
    ///
    /// No default: both implementors know the answer, and a default of "none" would let a decoder
    /// that starts recording flags have them silently dropped here.
    fn flags(&self) -> Option<&PixelFlags>;

    /// How many pixels hold each flag, for an image that carries any.
    fn flag_counts(&self) -> Option<&FlagCounts> {
        self.flags().map(PixelFlags::counts)
    }

    /// The image's channel planes, its flags and its flat gain, moved out.
    fn into_parts(self) -> ImageParts;

    /// The image `parts` make, of `cfa_type` and with `metadata`: a kept frame read back whole.
    fn from_parts(parts: ImageParts, cfa_type: Option<CfaType>, metadata: ImageMetadata) -> Self;

    /// The gain a flat applied to the image's pixels, for an image a flat divided: of its
    /// channels, or of a mosaic's colors.
    fn flat_gain(&self) -> Option<&FrameGain>;
}

/// What [`StackableImage::into_parts`] moves out of an image.
#[derive(Debug)]
pub(crate) struct ImageParts {
    pub(crate) planes: ArrayVec<Buffer2<f32>, 3>,
    pub(crate) flags: Option<PixelFlags>,
    pub(crate) flat_gain: Option<FrameGain>,
}
