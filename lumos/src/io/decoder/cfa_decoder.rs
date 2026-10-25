//! [`CfaDecoder`]: frames read as undemosaiced sensor images.

use std::path::Path;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::io::decoder::decoder_kind::DecoderKind;
use crate::io::decoder::{Decoder, Peek};
use crate::io::error::ImageError;
use crate::io::fits::decode::fits_file::FitsFile;
use crate::io::load_ctx::LoadCtx;
use crate::io::raw::raw_file::RawFile;

/// Frames read as [`CfaImage`]s: [`LoadCtx::cfa`].
#[derive(Debug)]
pub(crate) struct CfaDecoder;

/// What a sensor frame's header read leaves for its load, by the frame's format.
#[derive(Debug)]
pub(crate) enum CfaRead {
    Raw(RawFile),
    /// Boxed: a reader's buffers and scratch are several hundred bytes inline.
    Fits(Box<FitsFile>),
}

impl Decoder for CfaDecoder {
    type Image = CfaImage;
    type Read = CfaRead;
    const KIND: DecoderKind = DecoderKind::Cfa;

    fn load(path: &Path, ctx: &LoadCtx, gains: &SharedGains) -> Result<CfaImage, ImageError> {
        ctx.cfa_sharing(path, gains)
    }

    fn read_len(read: &CfaRead) -> usize {
        match read {
            CfaRead::Raw(file) => file.len(),
            CfaRead::Fits(file) => file.held_bytes(),
        }
    }

    fn load_read(
        read: CfaRead,
        path: &Path,
        ctx: &LoadCtx,
        gains: &SharedGains,
    ) -> Result<CfaImage, ImageError> {
        ctx.cfa_read(read, path, gains)
    }

    /// A RAW or FITS header answers without a decode; a RAW file is read whole for it.
    fn peek(path: &Path, ctx: &LoadCtx) -> Option<Peek<CfaRead>> {
        ctx.cfa_peek(path).ok()
    }
}
