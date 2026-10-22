//! [`CfaDecoder`]: frames read as undemosaiced sensor images.

use std::path::Path;

use crate::decode::decoder_kind::DecoderKind;
use crate::image::cfa::cfa_image::CfaImage;
use crate::io::decoder::Decoder;
use crate::io::error::ImageError;
use crate::io::frame_info::FrameInfo;
use crate::io::load_ctx::LoadCtx;

/// Frames read as [`CfaImage`]s: [`LoadCtx::cfa`].
#[derive(Debug)]
pub(crate) struct CfaDecoder;

impl Decoder for CfaDecoder {
    type Image = CfaImage;
    const KIND: DecoderKind = DecoderKind::Cfa;

    fn load(path: &Path, ctx: &LoadCtx) -> Result<CfaImage, ImageError> {
        ctx.cfa(path)
    }

    /// A RAW or FITS header answers without a decode.
    fn peek(path: &Path, ctx: &LoadCtx) -> Option<FrameInfo> {
        ctx.cfa_info(path).ok()
    }
}
