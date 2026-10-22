//! [`LinearDecoder`]: frames read as linear images.

use std::convert::Infallible;
use std::path::Path;

use crate::decode::decoder_kind::DecoderKind;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::io::decoder::{Decoder, Peek};
use crate::io::error::ImageError;
use crate::io::fits::decode as fits_decode;
use crate::io::input_format::InputFormat;
use crate::io::load_ctx::LoadCtx;

/// Frames read as [`LinearImage`]s: [`LoadCtx::linear`].
#[derive(Debug)]
pub(crate) struct LinearDecoder;

impl Decoder for LinearDecoder {
    type Image = LinearImage;
    type Read = Infallible;
    const KIND: DecoderKind = DecoderKind::Linear;

    fn load(path: &Path, ctx: &LoadCtx, gains: &SharedGains) -> Result<LinearImage, ImageError> {
        ctx.linear_sharing(path, gains)
    }

    fn read_len(read: &Infallible) -> usize {
        match *read {}
    }

    fn load_read(read: Infallible, _: &Path, _: &LoadCtx) -> Result<LinearImage, ImageError> {
        match read {}
    }

    /// A FITS header answers without a decode, the reader's chunk buffers with it. A float TIFF
    /// has no header this reads apart from its pixels.
    fn peek(path: &Path, ctx: &LoadCtx) -> Option<Peek<Infallible>> {
        match InputFormat::of(path).ok()? {
            InputFormat::Fits => fits_decode::linear_info(path, ctx)
                .ok()
                .map(|info| Peek { info, read: None }),
            InputFormat::CameraRaw | InputFormat::Raster(_) => None,
        }
    }
}
