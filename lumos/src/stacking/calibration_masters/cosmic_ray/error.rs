//! Why a cosmic-ray pass could not run on a frame.

use thiserror::Error;

/// The parametric noise model needs the ADU one sample unit is worth, which a frame states only
/// through the quantization σ its decoder recorded — a float FITS or a synthesized frame has none.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error(
    "parametric cosmic-ray noise needs the frame's ADC step, which its decoder did not record; \
     use the empirical noise model for this frame"
)]
pub struct UnknownAdcStep;
