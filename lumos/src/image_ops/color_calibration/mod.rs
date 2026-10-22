//! Color calibration: neutralize the per-channel sky background and remove the residual green cast
//! from a one-shot-color stack.
//!
//! - [`NeutralizeBg`] (linear, pre-stretch): estimate each channel's background and
//!   additively shift them to a common level, so the sky is neutral gray (R=G=B).
//! - [`Scnr`] (post-stretch): Subtractive Chromatic Noise Reduction — clamp green that exceeds the
//!   red/blue average, the residual green being noise on a color-balanced deep-sky image.
//!
//! [`NeutralizeBg`]: neutralize_background::NeutralizeBackground
//! [`Scnr`]: scnr::Scnr

pub(crate) mod neutralize_bg;
pub(crate) mod scnr;

#[cfg(test)]
mod tests;
