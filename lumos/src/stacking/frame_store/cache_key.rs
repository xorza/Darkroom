//! [`CacheKey`]: what a kept frame cache was decoded from and how, so a later run reuses only planes
//! it would have decoded the same way.

use common::FileIdentity;
use serde::{Deserialize, Serialize};

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

/// The digest the characterization tests pin for each decode a frame cache can hold — see
/// `testing::characterization`. They are bit patterns of an `x86_64` AVX2/FMA host.
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
}

pub(crate) const DECODE_PINS: DecodePins = DecodePins {
    fits_linear: "eef15741e2043c92",
    float_tiff: "9eb47b3ab90062b4",
    fits_cfa: "323b683215e2a8a2",
    raw_cfa: "b6144af28244502b",
};

/// The decode version every cache key carries: derived from [`DECODE_PINS`], not bumped by hand.
/// A change of any decoder's output fails its characterization test until the pin moves, and the
/// pin moving moves this, so a kept cache written by an older decoder is never reused.
pub(crate) const DECODE_VERSION: u64 = pins_fingerprint(&[
    DECODE_PINS.fits_linear,
    DECODE_PINS.float_tiff,
    DECODE_PINS.fits_cfa,
    DECODE_PINS.raw_cfa,
]);

/// FNV-1a over `pins`, each followed by a `0xff` no pin holds, so no two lists hash as one
/// concatenation. A version derived from pinned digests rather than a number bumped by hand.
pub(crate) const fn pins_fingerprint(pins: &[&str]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut pin = 0;
    while pin < pins.len() {
        let bytes = pins[pin].as_bytes();
        let mut byte = 0;
        while byte < bytes.len() {
            hash = (hash ^ bytes[byte] as u64).wrapping_mul(PRIME);
            byte += 1;
        }
        hash = (hash ^ 0xff).wrapping_mul(PRIME);
        pin += 1;
    }
    hash
}

/// What a cached frame's planes were decoded from and by.
///
/// The cache file names come from the source's canonical path and the [`DecoderKind`]; this is the
/// record beside them that says whether the planes under those names are still the ones a decode
/// now would give. The stacking load always decodes with the default FITS options, so the options
/// are not part of the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CacheKey {
    pub(crate) source: FileIdentity,
    pub(crate) decoder: DecoderKind,
    pub(crate) decode_version: u64,
}

impl CacheKey {
    pub(crate) const fn new(source: FileIdentity, decoder: DecoderKind) -> Self {
        Self {
            source,
            decoder,
            decode_version: DECODE_VERSION,
        }
    }
}
