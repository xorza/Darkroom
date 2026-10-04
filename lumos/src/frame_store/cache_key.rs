//! [`CacheKey`]: what a kept frame cache was decoded from and how, so a later run reuses only planes
//! it would have decoded the same way.

use common::{FileIdentity, SerdeFormat};
use serde::{Deserialize, Serialize};

use crate::io::image::load_context::LoadContext;

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

/// The digest the characterization tests pin for each decode a frame cache can hold, and for the
/// statistics it keeps beside the planes — see `internals::characterization`. They are bit
/// patterns of an `x86_64` AVX2/FMA host.
#[derive(Debug)]
pub(crate) struct DecodePins {
    /// [`LinearImage::from_file`](crate::LinearImage::from_file) on a FITS image.
    pub(crate) fits_linear: &'static str,
    /// [`LinearImage::from_file`](crate::LinearImage::from_file) on a floating-point TIFF.
    pub(crate) float_tiff: &'static str,
    /// [`CfaImage::from_file`](crate::CfaImage::from_file) on a mosaic FITS.
    pub(crate) fits_cfa: &'static str,
    /// [`CfaImage::from_file`](crate::CfaImage::from_file) on a camera RAW, pinned on the
    /// `real-data` dataset and checked only with that feature.
    pub(crate) raw_cfa: &'static str,
    /// [`CfaImage::demosaic`](crate::CfaImage), which
    /// [`LinearImage::from_file`](crate::LinearImage::from_file) runs on a camera RAW and on a
    /// mosaic FITS, of a calibrated Bayer light and of an X-Trans one at each pass count.
    pub(crate) demosaic: &'static str,
    /// `FrameStats::measure`, which a kept frame is committed with, on a mono, a mosaic and a
    /// demosaiced frame.
    pub(crate) frame_stats: &'static str,
}

pub(crate) const DECODE_PINS: DecodePins = DecodePins {
    fits_linear: "eef15741e2043c92",
    float_tiff: "029c2cc11944e24e",
    fits_cfa: "ab330c08a5153ba9",
    raw_cfa: "b6144af28244502b",
    demosaic: "355d39076b12df81",
    frame_stats: "612f3781e4aad045",
};

/// The decode version every cache key carries: derived from [`DECODE_PINS`], not bumped by hand.
/// A change of any decoder's output fails its characterization test until the pin moves, and the
/// pin moving moves this, so a kept cache written by an older decoder is never reused.
pub(crate) const DECODE_VERSION: u64 = pins_fingerprint(&[
    DECODE_PINS.fits_linear,
    DECODE_PINS.float_tiff,
    DECODE_PINS.fits_cfa,
    DECODE_PINS.raw_cfa,
    DECODE_PINS.demosaic,
    DECODE_PINS.frame_stats,
]);

/// FNV-1a 64's starting state.
pub(crate) const FNV1A_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64 continued from `hash` over `bytes`.
pub(crate) const fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut byte = 0;
    while byte < bytes.len() {
        hash = (hash ^ bytes[byte] as u64).wrapping_mul(PRIME);
        byte += 1;
    }
    hash
}

/// FNV-1a over `pins`, each followed by a `0xff` no pin holds, so no two lists hash as one
/// concatenation. A version derived from pinned digests rather than a number bumped by hand.
pub(crate) const fn pins_fingerprint(pins: &[&str]) -> u64 {
    let mut hash = FNV1A_OFFSET;
    let mut pin = 0;
    while pin < pins.len() {
        hash = fnv1a(fnv1a(hash, pins[pin].as_bytes()), &[0xff]);
        pin += 1;
    }
    hash
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
        // Every field of the options, the checksum policy too: a frame cached without a check
        // would otherwise pass for one that a required checksum admitted.
        let options = common::serialize(
            &(&context.fits, context.xtrans_passes),
            SerdeFormat::Bitcode,
        )
        .expect("decode options of plain enums and scalars always serialize");
        Self {
            source,
            decoder,
            decode_version: DECODE_VERSION,
            options: fnv1a(FNV1A_OFFSET, &options),
        }
    }
}
