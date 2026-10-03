//! Why a cosmic-ray pass could not run on a frame.

use thiserror::Error;

/// A stated gain needs the ADU one sample unit is worth, which a frame states through a declared
/// scale or the quantization σ its decoder recorded — a float FITS or a synthesized frame has
/// neither.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error(
    "a stated cosmic-ray gain needs the ADU one sample unit is worth, which this frame does not \
     record; use the measured noise model for it"
)]
pub struct UnknownAdcStep;
