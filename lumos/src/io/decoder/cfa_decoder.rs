//! [`CfaDecoder`]: frames read as undemosaiced sensor images.

use std::path::Path;

use crate::decode::decoder_kind::DecoderKind;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::io::decoder::{Decoder, Peek};
use crate::io::error::ImageError;
use crate::io::load_ctx::LoadCtx;
use crate::io::raw::raw_file::RawFile;

/// Frames read as [`CfaImage`]s: [`LoadCtx::cfa`].
#[derive(Debug)]
pub(crate) struct CfaDecoder;

impl Decoder for CfaDecoder {
    type Image = CfaImage;
    type Read = RawFile;
    const KIND: DecoderKind = DecoderKind::Cfa;

    fn load(path: &Path, ctx: &LoadCtx, gains: &SharedGains) -> Result<CfaImage, ImageError> {
        ctx.cfa_sharing(path, gains)
    }

    fn read_len(read: &RawFile) -> usize {
        read.len()
    }

    fn load_read(read: RawFile, path: &Path, ctx: &LoadCtx) -> Result<CfaImage, ImageError> {
        ctx.cfa_read(read, path)
    }

    /// A RAW or FITS header answers without a decode; a RAW file is read whole for it.
    fn peek(path: &Path, ctx: &LoadCtx) -> Option<Peek<RawFile>> {
        ctx.cfa_peek(path).ok()
    }
}
