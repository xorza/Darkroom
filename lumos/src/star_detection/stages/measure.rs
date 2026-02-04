//! Measurement stage: compute precise centroids and star properties.
//!
//! Takes detected regions and computes sub-pixel centroids, flux, FWHM,
//! and quality metrics for each candidate star.

use crate::common::Buffer2;

use super::super::background::BackgroundEstimate;
use super::super::centroid::compute_centroid;
use super::super::config::Config;
use super::super::region::Region;
use super::super::star::Star;

/// Measure precise centroids and properties for detected regions.
///
/// Computes sub-pixel positions, flux, FWHM, and quality metrics for each
/// region in parallel using rayon.
pub fn measure(
    regions: &[Region],
    pixels: &Buffer2<f32>,
    stats: &BackgroundEstimate,
    config: &Config,
) -> Vec<Star> {
    use rayon::prelude::*;

    regions
        .par_iter()
        .filter_map(|region| compute_centroid(pixels, stats, region, config))
        .collect()
}
