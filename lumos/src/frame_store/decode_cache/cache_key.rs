//! [`CacheKey`]: what a kept frame cache was decoded from and how, so a later run reuses only planes
//! it would have decoded the same way.

use common::FileIdentity;
use serde::{Deserialize, Serialize};

use crate::frame_store::decode_cache::decode_options::DecodeOptions;
use crate::frame_store::decode_cache::decode_pins::DECODE_VERSION;
use crate::io::load_context::LoadContext;

/// Which decoder produced a cached frame's planes.
///
/// A RAW frame decodes to a demosaiced image or to its undemosaiced sensor plane, and both can have
/// the same size on disk. Without this in the key, a mosaic load would map a demosaiced channel as
/// its sensor plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DecoderKind {
    /// [`LinearImage::from_file`](crate::LinearImage::from_file).
    Linear,
    /// [`CfaImage::from_file`](crate::CfaImage::from_file).
    Cfa,
}

impl DecoderKind {
    /// The byte this decoder contributes to a cache file name.
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Linear => 0,
            Self::Cfa => 1,
        }
    }
}

/// What a cached frame's planes were decoded from and by.
///
/// The cache file names come from the source's canonical path and the [`DecoderKind`]; this is the
/// record beside them that says whether the planes under those names are still the ones a decode
/// now would give: from the same file, by the same decoder at the same version, under the same
/// options — another HDU, null policy, float scale or X-Trans pass count decodes other planes from
/// the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CacheKey {
    pub(crate) source: FileIdentity,
    pub(crate) decoder: DecoderKind,
    pub(crate) decode_version: u64,
    /// FNV-1a of the decode options of the [`LoadContext`], as bitcode.
    pub(crate) options: u64,
}

impl CacheKey {
    pub(crate) fn new(source: FileIdentity, decoder: DecoderKind, context: &LoadContext) -> Self {
        Self {
            source,
            decoder,
            decode_version: DECODE_VERSION,
            options: DecodeOptions {
                fits: &context.fits,
                xtrans_passes: context.xtrans_passes,
            }
            .digest(),
        }
    }
}
