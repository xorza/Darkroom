//! FWHM estimation stage.
//!
//! Determines the effective FWHM for matched filtering by either using
//! a manual value, auto-estimating from bright stars, or disabling.

use crate::common::Buffer2;

use super::super::buffer_pool::BufferPool;
use super::super::candidate_detection::detect_stars;
use super::super::centroid::compute_centroid;
use super::super::config::Config;
use super::super::fwhm_estimation::{self, EffectiveFwhm, FwhmEstimate};
use super::super::image_stats::ImageStats;
use super::super::star::Star;

/// Determine the effective FWHM for matched filtering.
///
/// Returns:
/// - `EffectiveFwhm::Manual(fwhm)` if `config.expected_fwhm > 0`
/// - `EffectiveFwhm::Estimated(estimate)` if auto-estimation is enabled
/// - `EffectiveFwhm::Disabled` if matched filtering is disabled
pub fn estimate_fwhm(
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    config: &Config,
    pool: &mut BufferPool,
) -> EffectiveFwhm {
    // Manual FWHM takes precedence
    if config.expected_fwhm > f32::EPSILON {
        return EffectiveFwhm::Manual(config.expected_fwhm);
    }

    // Auto-estimate if enabled
    if config.auto_estimate_fwhm {
        let estimate = estimate_from_bright_stars(pixels, stats, config, pool);
        return EffectiveFwhm::Estimated(estimate);
    }

    EffectiveFwhm::Disabled
}

/// Perform first-pass detection and estimate FWHM from bright stars.
fn estimate_from_bright_stars(
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    config: &Config,
    pool: &mut BufferPool,
) -> FwhmEstimate {
    // Use stricter thresholds for FWHM estimation
    let first_pass_config = Config {
        sigma_threshold: config.sigma_threshold * config.fwhm_estimation_sigma_factor,
        expected_fwhm: 0.0,
        min_area: 3,
        min_snr: config.min_snr * 2.0,
        ..config.clone()
    };

    let candidates = detect_stars(pixels, None, stats, &first_pass_config, pool);
    tracing::debug!(
        "FWHM estimation: first pass detected {} bright star candidates",
        candidates.len()
    );

    let stars = compute_centroids(candidates, pixels, stats, &first_pass_config);

    fwhm_estimation::estimate_fwhm(
        &stars,
        config.min_stars_for_fwhm,
        4.0,
        config.max_eccentricity,
        config.max_sharpness,
    )
}

/// Compute centroids for star candidates in parallel.
fn compute_centroids(
    candidates: Vec<super::super::candidate_detection::StarCandidate>,
    pixels: &Buffer2<f32>,
    background: &ImageStats,
    config: &Config,
) -> Vec<Star> {
    use super::super::region::Region;
    use rayon::prelude::*;

    candidates
        .into_par_iter()
        .filter_map(|c| {
            let region = Region {
                bbox: c.bbox,
                peak: c.peak,
                peak_value: c.peak_value,
                area: c.area,
            };
            compute_centroid(pixels, background, &region, config)
        })
        .collect()
}
