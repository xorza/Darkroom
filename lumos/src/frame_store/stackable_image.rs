//! The image operations the shared frame store needs.

use std::fmt::Debug;
use std::path::Path;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame_store::cache_key::DecoderKind;
use crate::frame_store::frame_peek::FramePeek;
use crate::io::image::cfa::CfaType;
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::load_context::LoadContext;
use crate::io::image::null_mask::NullMask;

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

    /// Which of the image's pixels carry no measurement, for a source that declared any.
    ///
    /// No default: both implementors know the answer, and a default of "none" would let a decoder
    /// that starts recording nulls have them silently dropped here.
    fn nulls(&self) -> Option<&NullMask>;

    /// What a header alone settles about a frame, for a format that can answer without decoding.
    /// `None` when it cannot, which leaves the caller to decode the frame and read the answer off
    /// it — see [`FramePeek::of_decoded`].
    fn peek(_path: &Path, _context: &LoadContext) -> Option<FramePeek> {
        None
    }

    fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3>;
}
