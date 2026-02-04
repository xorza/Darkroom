//! Detection stage: threshold, label, deblend, extract regions.
//!
//! Combines matched filtering (optional), thresholding, connected component
//! labeling, and deblending into a single stage that returns detected regions.

use crate::common::Buffer2;

use super::super::buffer_pool::BufferPool;
use super::super::candidate_detection::{self, StarCandidate};
use super::super::config::Config;
use super::super::convolution::matched_filter;
use super::super::image_stats::ImageStats;
use super::super::region::Region;

/// Detect star candidate regions in the image.
///
/// Applies matched filtering if FWHM is provided, then performs thresholding,
/// connected component labeling, and deblending to extract candidate regions.
///
/// All buffer management is contained within this function.
pub fn detect(
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    fwhm: Option<f32>,
    config: &Config,
    pool: &mut BufferPool,
) -> Vec<Region> {
    let mut scratch = pool.acquire_f32();

    // Apply matched filter if FWHM is provided
    let filtered: Option<&Buffer2<f32>> = if let Some(fwhm) = fwhm {
        tracing::debug!(
            "Applying matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.1}°",
            fwhm,
            config.psf_axis_ratio,
            config.psf_angle.to_degrees()
        );

        let mut convolution_scratch = pool.acquire_f32();
        let mut convolution_temp = pool.acquire_f32();
        matched_filter(
            pixels,
            &stats.background,
            fwhm,
            config.psf_axis_ratio,
            config.psf_angle,
            &mut scratch,
            &mut convolution_scratch,
            &mut convolution_temp,
        );
        pool.release_f32(convolution_temp);
        pool.release_f32(convolution_scratch);

        Some(&scratch)
    } else {
        None
    };

    // Detect star candidates using existing logic
    let candidates = candidate_detection::detect_stars(pixels, filtered, stats, config, pool);

    pool.release_f32(scratch);

    // Convert StarCandidate to Region
    candidates.into_iter().map(candidate_to_region).collect()
}

/// Convert a StarCandidate to a Region.
#[inline]
fn candidate_to_region(c: StarCandidate) -> Region {
    Region {
        bbox: c.bbox,
        peak: c.peak,
        peak_value: c.peak_value,
        area: c.area,
    }
}
