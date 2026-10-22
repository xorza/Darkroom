//! [`CacheKey`]: what a kept frame cache was decoded from and how, so a later run reuses only planes
//! it would have decoded the same way.

use common::FileIdentity;
use serde::{Deserialize, Serialize};

use crate::decode::decode_params::DecodeParams;
use crate::decode::decode_pins::DECODE_VERSION;
use crate::decode::decoder_kind::DecoderKind;
use crate::io::load_ctx::LoadCtx;

/// What a cached frame's planes were decoded from and by.
///
/// The cache file names come from the source's canonical path and this key, which a commit records
/// beside the planes too: they are the ones a decode now would give only from the same file, by the
/// same decoder at the same version, under the same options — another HDU, null policy, float
/// scale or X-Trans pass count decodes other planes from the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CacheKey {
    pub(crate) source: FileIdentity,
    pub(crate) decoder: DecoderKind,
    pub(crate) decode_version: u64,
    /// FNV-1a of the decode options of the [`LoadCtx`], as bitcode.
    pub(crate) options: u64,
}

impl CacheKey {
    pub(crate) fn new(source: FileIdentity, decoder: DecoderKind, ctx: &LoadCtx) -> Self {
        Self {
            source,
            decoder,
            decode_version: DECODE_VERSION,
            options: DecodeParams {
                fits: &ctx.fits,
                nulls: ctx.nulls,
                xtrans_passes: ctx.xtrans_passes,
            }
            .digest(),
        }
    }
}
