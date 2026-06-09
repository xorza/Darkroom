//! Pipeline stages for star detection.
//!
//! Each stage is a pure function that transforms data, with all buffer
//! management contained within.

pub(crate) mod detect;
#[cfg(test)]
pub(crate) mod detect_test_utils;
pub(crate) mod filter;
pub(crate) mod fwhm;
pub(crate) mod measure;
pub(crate) mod prepare;

/// Floor for a MAD-scaled FWHM rejection threshold, as a fraction of the median FWHM.
/// Prevents a zero threshold when the FWHM distribution is near-uniform (MAD ≈ 0).
/// Shared by the `fwhm` (estimation) and `filter` (outlier-culling) stages.
pub(crate) const FWHM_MAD_FLOOR_FRACTION: f32 = 0.1;
