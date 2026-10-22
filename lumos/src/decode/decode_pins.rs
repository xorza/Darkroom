//! [`DecodePins`]: the digests every decode a frame cache can hold is pinned to, and the decode
//! version derived from them.

use crate::decode::fnv1a;

/// The digest the characterization tests pin for each decode a frame cache can hold, and for the
/// statistics it keeps beside the planes — see `internals::characterization`. They are bit
/// patterns of an `x86_64` AVX2/FMA host.
#[derive(Debug)]
pub(crate) struct DecodePins {
    /// [`LoadCtx::linear`](crate::LoadCtx::linear) on a FITS image.
    pub(crate) fits_linear: &'static str,
    /// [`LoadCtx::linear`](crate::LoadCtx::linear) on a floating-point TIFF.
    pub(crate) float_tiff: &'static str,
    /// [`LoadCtx::cfa`](crate::LoadCtx::cfa) on a mosaic FITS — its plane, flags
    /// and quantization σ.
    pub(crate) fits_cfa: &'static str,
    /// [`LoadCtx::cfa`](crate::LoadCtx::cfa) on a camera RAW — its plane, flags
    /// and quantization σ — pinned on the `real-data` dataset and checked only with that feature.
    pub(crate) raw_cfa: &'static str,
    /// [`CfaImage::demosaic`](crate::CfaImage), which
    /// [`LoadCtx::linear`](crate::LoadCtx::linear) runs on a camera RAW and on a
    /// mosaic FITS, of a calibrated Bayer light and of an X-Trans one at each pass count.
    pub(crate) demosaic: &'static str,
    /// `FrameStats::measure`, which a kept frame is committed with, on a mono, a mosaic and a
    /// demosaiced frame.
    pub(crate) frame_stats: &'static str,
}

pub(crate) const DECODE_PINS: DecodePins = DecodePins {
    fits_linear: "eef15741e2043c92",
    float_tiff: "029c2cc11944e24e",
    fits_cfa: "5fa56a9f2b1e6153",
    // Not a digest: the RAW decode's flags and quantization σ changed with no dataset at hand to
    // pin them on. A placeholder still moves `DECODE_VERSION`, so no cache of the old flags is
    // reused, and the `real-data` snapshot prints the digest to pin here.
    raw_cfa: "repin-on-real-data",
    demosaic: "daf5bebe6f990b05",
    frame_stats: "34c85e5fa3518fb1",
};

/// The decode version every cache key carries: derived from [`DECODE_PINS`], not bumped by hand.
/// A change of any decoder's output fails its characterization test until the pin moves, and the
/// pin moving moves this, so a kept cache written by an older decoder is never reused.
pub(crate) const DECODE_VERSION: u64 = fnv1a::fingerprint(&[
    DECODE_PINS.fits_linear,
    DECODE_PINS.float_tiff,
    DECODE_PINS.fits_cfa,
    DECODE_PINS.raw_cfa,
    DECODE_PINS.demosaic,
    DECODE_PINS.frame_stats,
]);
