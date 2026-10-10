//! The image operations the shared frame store needs.

use std::fmt::Debug;
use std::path::Path;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame_store::cache_key::DecoderKind;
use crate::frame_store::frame_peek::FramePeek;
use crate::io::image::cfa::CfaType;
use crate::io::image::error::ImageError;
use crate::io::image::flat_gain::FlatGain;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::load_context::LoadContext;
use crate::io::image::pixel_flags::PixelFlags;

/// Image operations needed by the shared frame store.
pub(crate) trait StackableImage: Send + Sync + Debug + Sized {
    /// The decoder [`Self::load`] runs, which a kept cache records beside the planes it wrote.
    const DECODER: DecoderKind;

    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, channel: usize) -> &[f32];
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
            .map_or(dimensions.channels(), |cfa| cfa.num_colors());
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
