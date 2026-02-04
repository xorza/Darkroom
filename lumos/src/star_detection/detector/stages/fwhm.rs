//! FWHM estimation stage.
//!
//! Determines the effective FWHM for matched filtering by either using
//! a manual value, auto-estimating from bright stars, or disabling.

use crate::common::Buffer2;
use crate::math::{mad_f32_with_scratch, median_f32_mut};

use super::super::super::background::BackgroundEstimate;
use super::super::super::buffer_pool::BufferPool;
use super::super::super::centroid::measure_star;
use super::super::super::config::Config;
use super::super::super::deblend::Region;
use super::super::super::star::Star;
use super::detect::detect;

/// Result of FWHM estimation stage.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FwhmResult {
    /// FWHM value if matched filtering should be used, None if disabled.
    pub fwhm: Option<f32>,
    /// Number of stars used for auto-estimation (0 if manual or disabled).
    pub stars_used: usize,
}

/// Determine the effective FWHM for matched filtering.
///
/// Returns:
/// - `fwhm: Some(value)` if manual FWHM is set or auto-estimation succeeds
/// - `fwhm: None` if matched filtering is disabled
/// - `stars_used` is non-zero only when auto-estimation was performed
pub(crate) fn estimate_fwhm(
    pixels: &Buffer2<f32>,
    stats: &BackgroundEstimate,
    config: &Config,
    pool: &mut BufferPool,
) -> FwhmResult {
    // Manual FWHM takes precedence
    if config.expected_fwhm > f32::EPSILON {
        return FwhmResult {
            fwhm: Some(config.expected_fwhm),
            stars_used: 0,
        };
    }

    // Auto-estimate if enabled
    if config.auto_estimate_fwhm {
        return estimate_from_bright_stars(pixels, stats, config, pool);
    }

    FwhmResult {
        fwhm: None,
        stars_used: 0,
    }
}

/// Perform first-pass detection and estimate FWHM from bright stars.
fn estimate_from_bright_stars(
    pixels: &Buffer2<f32>,
    stats: &BackgroundEstimate,
    config: &Config,
    pool: &mut BufferPool,
) -> FwhmResult {
    // Use stricter thresholds for FWHM estimation
    let first_pass_config = Config {
        sigma_threshold: config.sigma_threshold * config.fwhm_estimation_sigma_factor,
        expected_fwhm: 0.0,
        min_area: 3,
        min_snr: config.min_snr * 2.0,
        ..config.clone()
    };

    // Run detection without matched filter
    let regions = detect(pixels, stats, None, &first_pass_config, pool).regions;
    tracing::debug!(
        "FWHM estimation: first pass detected {} bright star candidates",
        regions.len()
    );

    let stars = compute_centroids(&regions, pixels, stats, &first_pass_config);

    estimate_fwhm_from_stars(
        &stars,
        config.min_stars_for_fwhm,
        4.0,
        config.max_eccentricity,
        config.max_sharpness,
    )
}

/// Compute centroids for star candidates in parallel.
fn compute_centroids(
    regions: &[Region],
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    config: &Config,
) -> Vec<Star> {
    use rayon::prelude::*;

    regions
        .par_iter()
        .filter_map(|region| measure_star(pixels, background, region, config))
        .collect()
}

/// Estimate FWHM from a set of detected stars.
///
/// Uses robust statistics (median + MAD) to handle outliers from
/// cosmic rays, saturated stars, and edge artifacts.
///
/// # Algorithm
/// 1. Filter stars by quality (not saturated, reasonable eccentricity, positive FWHM, not cosmic ray)
/// 2. Compute median FWHM from filtered stars
/// 3. Reject outliers using MAD-based threshold (keep within 3×MAD of median)
/// 4. Recompute median from remaining stars
fn estimate_fwhm_from_stars(
    stars: &[Star],
    min_stars: usize,
    default_fwhm: f32,
    max_eccentricity: f32,
    max_sharpness: f32,
) -> FwhmResult {
    // Filter stars for quality and collect FWHM values
    let mut fwhms: Vec<f32> = stars
        .iter()
        .filter(|s| {
            !s.is_saturated()
                && s.eccentricity <= max_eccentricity
                && s.sharpness < max_sharpness
                && (0.5..20.0).contains(&s.fwhm)
        })
        .map(|s| s.fwhm)
        .collect();

    if fwhms.len() < min_stars {
        tracing::debug!(
            "Insufficient stars for FWHM estimation: {} < {}, using default {:.1}",
            fwhms.len(),
            min_stars,
            default_fwhm
        );
        return FwhmResult {
            fwhm: Some(default_fwhm),
            stars_used: fwhms.len(),
        };
    }

    // Scratch buffer for MAD computation
    let mut scratch = Vec::with_capacity(fwhms.len());

    // Compute median and MAD for outlier rejection
    let median = median_f32_mut(&mut fwhms);
    let mad = mad_f32_with_scratch(&fwhms, median, &mut scratch);

    // Reject outliers: keep within 3×MAD of median (with floor for uniform distributions)
    let threshold = 3.0 * mad.max(median * 0.1);
    let count_before = fwhms.len();
    fwhms.retain(|&f| (f - median).abs() <= threshold);

    // If too many rejected, use pre-rejection median
    if fwhms.len() < min_stars {
        tracing::debug!(
            "Too many outliers rejected ({count_before} -> {}), using pre-rejection median {median:.2}",
            fwhms.len(),
        );
        return FwhmResult {
            fwhm: Some(median),
            stars_used: fwhms.len(),
        };
    }

    // Final estimate from filtered stars
    let final_median = median_f32_mut(&mut fwhms);
    let final_mad = mad_f32_with_scratch(&fwhms, final_median, &mut scratch);

    tracing::info!(
        "Estimated FWHM: {final_median:.2} pixels (MAD: {final_mad:.2}, from {} stars)",
        fwhms.len()
    );

    FwhmResult {
        fwhm: Some(final_median),
        stars_used: fwhms.len(),
    }
}
