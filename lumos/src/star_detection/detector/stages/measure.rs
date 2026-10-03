//! Measurement stage: compute precise centroids and star properties.
//!
//! Takes detected regions and computes sub-pixel centroids, flux, FWHM,
//! and quality metrics for each candidate star, on the measurement plane.

use crate::star_detection::centroid::stamp::StampGrid;
use crate::star_detection::centroid::{compute_stamp_radius, measure_star};
use crate::star_detection::config::measurement_config::MeasurementConfig;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::detector::stages::prepared_frame::PreparedFrame;
use crate::star_detection::star::Star;

/// The width a measurement assumes when no FWHM is known: zero, which `compute_stamp_radius` and
/// the moments window clamp up to their smallest stamp and narrowest window — the choice that
/// assumes nothing about the star beyond its being at least that wide.
const UNKNOWN_FWHM: f32 = 0.0;

/// Measure precise centroids and properties for detected regions on `frame`'s measurement plane,
/// flagging those whose peak pixel is saturated. `expected_fwhm` sizes every stamp; `None`
/// measures at [`UNKNOWN_FWHM`].
///
/// Computes sub-pixel positions, flux, FWHM, and quality metrics for each
/// region in parallel using rayon.
pub(crate) fn measure(
    regions: &[Region],
    frame: &PreparedFrame,
    config: &MeasurementConfig,
    expected_fwhm: Option<f32>,
) -> Vec<Star> {
    use rayon::prelude::*;

    let expected_fwhm = expected_fwhm.unwrap_or(UNKNOWN_FWHM);

    // One grid for the whole detection: `expected_fwhm` fixes the stamp radius, so every
    // candidate below fits over the same coordinates.
    let grid = StampGrid::new(compute_stamp_radius(expected_fwhm));

    regions
        .par_iter()
        .filter_map(|region| {
            measure_star(
                &frame.measure,
                &frame.sky,
                &frame.saturation,
                region,
                config,
                expected_fwhm,
                &grid,
            )
        })
        .collect()
}
