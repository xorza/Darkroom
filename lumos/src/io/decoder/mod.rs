//! [`Decoder`]: how the frames of a set are read from their files, as one image type.

pub(crate) mod cfa_decoder;
pub(crate) mod linear_decoder;

use std::path::Path;

use crate::decode::decoder_kind::DecoderKind;
use crate::image::stackable_image::StackableImage;
use crate::io::error::ImageError;
use crate::io::frame_info::FrameInfo;
use crate::io::load_context::LoadContext;

/// How a frame set's files decode, as one image type: what a run generic over its frames' type
/// reads them through.
pub(crate) trait Decoder {
    type Image: StackableImage;

    /// The decoder [`Self::load`] runs, which a kept cache records beside the planes it wrote.
    const KIND: DecoderKind;

    fn load(path: &Path, context: &LoadContext) -> Result<Self::Image, ImageError>;

    /// What a header alone settles about a frame, for a format that can answer without decoding.
    /// `None` when it cannot, which leaves the caller to decode the frame and read the answer off
    /// it.
    fn peek(path: &Path, context: &LoadContext) -> Option<FrameInfo>;
}
