//! Measurement stage: compute precise centroids and star properties.
//!
//! Takes detected regions and computes sub-pixel centroids, flux, FWHM,
//! and quality metrics for each candidate star.

use imaginarium::Buffer2;

use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use crate::stacking::star_detection::centroid::measure_star;
use crate::stacking::star_detection::config::MeasurementConfig;
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::star::Star;

/// Measure precise centroids and properties for detected regions.
///
/// Computes sub-pixel positions, flux, FWHM, and quality metrics for each
/// region in parallel using rayon.
pub(crate) fn measure(
    regions: &[Region],
    pixels: &Buffer2<f32>,
    stats: &BackgroundEstimate,
    config: &MeasurementConfig,
    expected_fwhm: f32,
) -> Vec<Star> {
    use rayon::prelude::*;

    regions
        .par_iter()
        .filter_map(|region| measure_star(pixels, stats, region, config, expected_fwhm))
        .collect()
}
