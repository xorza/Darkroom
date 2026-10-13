//! The image operations the shared frame store needs.

use std::fmt::Debug;
use std::path::Path;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame::frame_peek::FramePeek;
use crate::frame_store::decode_cache::cache_key::DecoderKind;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::pixel_flags::PixelFlags;
use crate::io::error::ImageError;
use crate::io::load_context::LoadContext;

/// Image operations needed by the shared frame store.
pub(crate) trait StackableImage: Send + Sync + Debug + Sized {
    /// The decoder [`Self::load`] runs, which a kept cache records beside the planes it wrote.
    const DECODER: DecoderKind;

    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, channel: usize) -> &Buffer2<f32>;
    fn metadata(&self) -> &ImageMetadata;
    /// The mosaic pattern of an undemosaiced sensor frame; `None` for any other image.
    fn cfa_type(&self) -> Option<CfaType>;
    fn load(path: &Path, context: &LoadContext) -> Result<Self, ImageError>;

    /// The image's data-quality flags, for an image that carries any.
    ///
    /// No default: both implementors know the answer, and a default of "none" would let a decoder
    /// that starts recording flags have them silently dropped here.
    fn flags(&self) -> Option<&PixelFlags>;

    /// What a header alone settles about a frame, for a format that can answer without decoding.
    /// `None` when it cannot, which leaves the caller to decode the frame and read the answer off
    /// it — see [`FramePeek::of_decoded`].
    fn peek(_path: &Path, _context: &LoadContext) -> Option<FramePeek> {
        None
    }

    /// The image's channel planes and its flags, moved out.
    fn into_parts(self) -> ImageParts;

    /// The image `parts` make, of `cfa_type` and with `metadata`: a kept frame read back whole.
    fn from_parts(parts: ImageParts, cfa_type: Option<CfaType>, metadata: ImageMetadata) -> Self;

    /// The gain a flat applied to the image's pixels, for an image a flat divided.
    ///
    /// # Panics
    ///
    /// If the gain covers another image: another size, or another count of colours or channels.
    /// Calibration sets it on the frame it divides and a warp replaces it with its own, so a
    /// mismatch is metadata moved between images by the caller.
    fn flat_gain(&self) -> Option<&FlatGain> {
        let gain = self.metadata().flat_gain.as_deref()?;
        let dimensions = self.dimensions();
        let channels = self
            .cfa_type()
            .map_or_else(|| dimensions.channels(), |cfa| cfa.num_colors());
        assert!(
            gain.size() == dimensions.size() && gain.channels() == channels,
            "an image's flat gain covers another image's pixels"
        );
        Some(gain)
    }
}

/// What [`StackableImage::into_parts`] moves out of an image.
#[derive(Debug)]
pub(crate) struct ImageParts {
    pub(crate) planes: ArrayVec<Buffer2<f32>, 3>,
    pub(crate) flags: Option<PixelFlags>,
}
