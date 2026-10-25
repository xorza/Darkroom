//! [`LinearDecoder`]: frames read as linear images.

use std::path::Path;

use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::io::decoder::decoder_kind::DecoderKind;
use crate::io::decoder::{Decoder, Peek};
use crate::io::error::ImageError;
use crate::io::fits::decode as fits_decode;
use crate::io::fits::decode::fits_file::FitsFile;
use crate::io::input_format::InputFormat;
use crate::io::load_ctx::LoadCtx;

/// Frames read as [`LinearImage`]s: [`LoadCtx::linear`].
#[derive(Debug)]
pub(crate) struct LinearDecoder;

impl Decoder for LinearDecoder {
    type Image = LinearImage;
    type Read = FitsFile;
    const KIND: DecoderKind = DecoderKind::Linear;

    fn load(path: &Path, ctx: &LoadCtx, gains: &SharedGains) -> Result<LinearImage, ImageError> {
        ctx.linear_sharing(path, gains)
    }

    fn read_len(read: &FitsFile) -> usize {
        read.held_bytes()
    }

    fn load_read(
        read: FitsFile,
        path: &Path,
        ctx: &LoadCtx,
        gains: &SharedGains,
    ) -> Result<LinearImage, ImageError> {
        ctx.linear_read(read, path, gains)
    }

    /// A FITS header answers without a decode, and its open file goes to the load. A float TIFF
    /// has no header this reads apart from its pixels.
    fn peek(path: &Path, ctx: &LoadCtx) -> Option<Peek<FitsFile>> {
        match InputFormat::of(path).ok()? {
            InputFormat::Fits => fits_decode::linear_peek(path, ctx).ok(),
            InputFormat::CameraRaw | InputFormat::Raster(_) => None,
        }
    }
}
