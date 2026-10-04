//! Measurement stage: compute precise centroids and star properties.
//!
//! Takes detected regions and computes sub-pixel centroids, flux, FWHM,
//! and quality metrics for each candidate star, on the measurement plane.

use crate::star_detection::centroid::measure_grid::MeasureGrid;
use crate::star_detection::centroid::{MeasurePlanes, measure_star};
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

    // One grid for the whole detection: `expected_fwhm` fixes the stamp, the window and the
    // annulus, so every candidate below is measured over the same ones.
    let grid = MeasureGrid::new(expected_fwhm);

    regions
        .par_iter()
        .filter_map(|region| {
            measure_star(
                MeasurePlanes {
                    residual: &frame.measure,
                    sky: &frame.sky,
                    saturation: &frame.saturation,
                    no_data: frame.no_data.as_ref(),
                },
                region,
                config,
                &grid,
            )
        })
        .collect()
}
