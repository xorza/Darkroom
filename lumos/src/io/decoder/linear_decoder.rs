//! [`LinearDecoder`]: frames read as linear images.

use std::path::Path;

use crate::decode::decoder_kind::DecoderKind;
use crate::image::linear_image::LinearImage;
use crate::io::decoder::Decoder;
use crate::io::error::ImageError;
use crate::io::fits::decode as fits_decode;
use crate::io::frame_info::FrameInfo;
use crate::io::input_format::InputFormat;
use crate::io::load_context::LoadContext;

/// Frames read as [`LinearImage`]s: [`LoadContext::linear`].
#[derive(Debug)]
pub(crate) struct LinearDecoder;

impl Decoder for LinearDecoder {
    type Image = LinearImage;
    const KIND: DecoderKind = DecoderKind::Linear;

    fn load(path: &Path, context: &LoadContext) -> Result<LinearImage, ImageError> {
        context.linear(path)
    }

    /// A FITS header answers without a decode, the reader's chunk buffers with it. A float TIFF
    /// has no header this reads apart from its pixels.
    fn peek(path: &Path, context: &LoadContext) -> Option<FrameInfo> {
        match InputFormat::of(path).ok()? {
            InputFormat::Fits => fits_decode::linear_info(path, context).ok(),
            InputFormat::CameraRaw | InputFormat::Raster(_) => None,
        }
    }
}
