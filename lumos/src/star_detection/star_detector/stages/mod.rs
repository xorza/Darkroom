//! Pipeline stages for star detection, in the order a detection runs them: the prepared frame,
//! which also estimates the FWHM, builds the detection planes and measures the stars; detection of
//! the candidates; and filtering. Each takes the detector's resources and returns its own counts.

pub(crate) mod detect;
pub(crate) mod filter;
pub(crate) mod fwhm;
pub(crate) mod prepared_frame;

use crate::math::statistics;

/// Floor for a MAD-scaled FWHM rejection threshold, as a fraction of the median FWHM.
/// Prevents a zero threshold when the FWHM distribution is near-uniform (MAD ≈ 0).
const FWHM_MAD_FLOOR_FRACTION: f32 = 0.1;

/// The centre and spread of a set of FWHMs, which both FWHM clips — the estimate's and the
/// filter's — are stated in: the median, and the σ the MAD states.
#[derive(Debug, Clone, Copy)]
struct FwhmSpread {
    median: f32,
    sigma: f32,
}

impl FwhmSpread {
    /// The spread of `values`, reordered, with `deviations` as scratch: the MAD floored at
    /// [`FWHM_MAD_FLOOR_FRACTION`] of the median, so a uniform set still clips at a width, then
    /// carried to σ for the count.
    fn of(values: &mut [f32], deviations: &mut Vec<f32>) -> Self {
        let median = statistics::median_mut(values);
        let mad = statistics::mad_with_scratch(values, median, deviations);
        Self {
            median,
            sigma: statistics::sigma_of_mad(
                statistics::mad_floored(mad, median, FWHM_MAD_FLOOR_FRACTION),
                values.len(),
            ),
        }
    }
}
