//! Why a cosmic-ray pass could not run on a frame.

use thiserror::Error;

/// Why a cosmic-ray pass refused a frame. The frame is left as it was.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CosmicRayError {
    /// A stated gain needs the ADU one sample unit is worth, which a frame states through a
    /// declared scale or the quantization σ its decoder recorded — a float FITS or a synthesized
    /// frame has neither.
    #[error(
        "a stated cosmic-ray gain needs the ADU one sample unit is worth, which this frame does \
         not record; use the measured noise model for it"
    )]
    UnknownAdcStep,
    /// The frame is an X-Trans mosaic. Its red and blue photosites have no point-symmetric
    /// same-colour pair near enough for a second difference, so no statistic tells a hit from a
    /// star's wing there, and no reference tool runs L.A.Cosmic on one.
    #[error(
        "single-frame cosmic-ray rejection does not run on X-Trans mosaics; rely on dithering and \
         stack-time rejection"
    )]
    XTransUnsupported,
}
