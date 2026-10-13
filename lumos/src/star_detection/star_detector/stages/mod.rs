//! Pipeline stages for star detection, in the order a detection runs them: the prepared frame,
//! which also measures the stars, FWHM estimation, detection, and filtering. Each leases its planes
//! and scratch from the detector's resources.

pub(crate) mod detect;
pub(crate) mod filter;
pub(crate) mod fwhm;
pub(crate) mod prepared_frame;

/// Floor for a MAD-scaled FWHM rejection threshold, as a fraction of the median FWHM.
/// Prevents a zero threshold when the FWHM distribution is near-uniform (MAD ≈ 0).
/// Shared by the `fwhm` (estimation) and `filter` (outlier-culling) stages.
const FWHM_MAD_FLOOR_FRACTION: f32 = 0.1;
