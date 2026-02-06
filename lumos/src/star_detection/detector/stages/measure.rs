//! Measurement stage: compute precise centroids and star properties.
//!
//! Takes detected regions and computes sub-pixel centroids, flux, FWHM,
//! and quality metrics for each candidate star.

use crate::common::Buffer2;

use crate::star_detection::background::BackgroundEstimate;
use crate::star_detection::centroid::measure_star;
use crate::star_detection::config::Config;
use crate::star_detection::deblend::Region;
use crate::star_detection::star::Star;

/// Measure precise centroids and properties for detected regions.
///
/// Computes sub-pixel positions, flux, FWHM, and quality metrics for each
/// region in parallel using rayon.
pub(crate) fn measure(
    regions: &[Region],
    pixels: &Buffer2<f32>,
    stats: &BackgroundEstimate,
    config: &Config,
) -> Vec<Star> {
    use rayon::prelude::*;

    regions
        .par_iter()
        .filter_map(|region| measure_star(pixels, stats, region, config))
        .collect()
}
