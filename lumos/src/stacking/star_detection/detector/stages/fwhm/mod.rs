//! FWHM estimation stage.
//!
//! Determines the effective FWHM for matched filtering by either using
//! a manual value, auto-estimating from bright stars, or disabling.

use crate::math::statistics::{mad_f32_with_scratch, mad_floored, median_f32_mut};
use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::config::detection_config::DetectionConfig;
use crate::stacking::star_detection::config::filter_config::FilterConfig;
use crate::stacking::star_detection::config::fwhm_config::FwhmConfig;
use crate::stacking::star_detection::detector::stages::FWHM_MAD_FLOOR_FRACTION;
use crate::stacking::star_detection::detector::stages::detect::detect;
use crate::stacking::star_detection::detector::stages::measure;
use crate::stacking::star_detection::resources::DetectionResources;
use crate::stacking::star_detection::star::{SATURATION_PEAK, Star};
use imaginarium::Buffer2;

/// Minimum plausible FWHM in pixels. Stars narrower than this are likely
/// cosmic rays or hot pixels.
const FWHM_MIN: f32 = 0.5;

/// Maximum plausible FWHM in pixels. Sources broader than this are likely
/// galaxies, nebulae, or artifacts rather than point sources.
const FWHM_MAX: f32 = 20.0;

/// Default FWHM used when auto-estimation has insufficient stars.
const DEFAULT_FWHM: f32 = 4.0;

/// MAD multiplier for outlier rejection in FWHM estimation.
/// Stars with FWHM deviating more than this many MADs from the median are rejected.
const FWHM_MAD_MULTIPLIER: f32 = 3.0;

/// Result of FWHM estimation stage.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FwhmResult {
    /// FWHM value if matched filtering should be used, None if disabled.
    pub(crate) fwhm: Option<f32>,
    /// Number of stars that actually contributed to `fwhm`'s value: 0 if manual,
    /// disabled, or auto-estimation fell back to a value with no star provenance —
    /// so non-zero means exactly "fwhm came from a genuine auto-estimate".
    pub(crate) stars_used: usize,
}

/// Determine the effective FWHM for matched filtering.
///
/// Precedence: auto-estimation (if enabled) wins, falling back to `expected_fwhm` (else a default)
/// when too few stars are found; otherwise the fixed `expected_fwhm` is used.
///
/// Returns:
/// - `fwhm: Some(value)` if auto-estimation runs or a fixed `expected_fwhm` is set
/// - `fwhm: None` if matched filtering is disabled (auto off and `expected_fwhm == 0`)
/// - `stars_used` is non-zero only when auto-estimation was performed
pub(crate) fn estimate_fwhm(
    pixels: &Buffer2<f32>,
    stats: &BackgroundEstimate,
    config: &Config,
    pool: &mut DetectionResources,
) -> FwhmResult {
    // Auto-estimation takes precedence; `expected` becomes its fallback when too few stars
    // are found (see `estimate_fwhm_from_stars`).
    if config.fwhm.auto_estimate {
        return estimate_from_bright_stars(pixels, stats, config, pool);
    }

    // Otherwise use the fixed expected FWHM (0 disables the matched filter).
    if config.fwhm.expected > f32::EPSILON {
        return FwhmResult {
            fwhm: Some(config.fwhm.expected),
            stars_used: 0,
        };
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
    pool: &mut DetectionResources,
) -> FwhmResult {
    let first_pass_config = DetectionConfig {
        sigma_threshold: config.detection.sigma_threshold * config.fwhm.estimation_sigma_factor,
        min_area: 3,
        ..config.detection.clone()
    };

    // Run detection without matched filter
    let regions = detect(pixels, stats, None, &first_pass_config, pool).regions;
    tracing::debug!(
        "FWHM estimation: first pass detected {} bright star candidates",
        regions.len()
    );

    let stars = measure::measure(&regions, pixels, stats, &config.measurement, 0.0);

    estimate_fwhm_from_stars(&stars, &config.fwhm, &config.filter)
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
    fwhm_config: &FwhmConfig,
    filter_config: &FilterConfig,
) -> FwhmResult {
    let min_stars = fwhm_config.min_stars;

    // Filter stars for quality and collect FWHM values
    let mut fwhms: Vec<f32> = stars
        .iter()
        .filter(|s| {
            !s.is_saturated(SATURATION_PEAK)
                && s.eccentricity <= filter_config.max_eccentricity
                && s.sharpness < filter_config.max_sharpness
                && (FWHM_MIN..FWHM_MAX).contains(&s.fwhm)
        })
        .map(|s| s.fwhm)
        .collect();

    if fwhms.len() < min_stars {
        // Fall back to the configured `expected` (a tuned per-preset seed); only use the
        // generic default if no expected FWHM was set.
        let fallback_fwhm = if fwhm_config.expected > f32::EPSILON {
            fwhm_config.expected
        } else {
            DEFAULT_FWHM
        };
        tracing::debug!(
            "Insufficient stars for FWHM estimation: {} < {}, using fallback {:.1}",
            fwhms.len(),
            min_stars,
            fallback_fwhm
        );
        // `fallback_fwhm` has no dependence on `fwhms` (it's `expected_fwhm` or the
        // hardcoded default), so `stars_used` must report 0, not the quality-passing
        // count — that zero is what makes this a `FwhmSource::Configured` downstream
        // rather than an `Estimated` that never measured anything.
        return FwhmResult {
            fwhm: Some(fallback_fwhm),
            stars_used: 0,
        };
    }

    // Scratch buffer for MAD computation
    let mut scratch = Vec::with_capacity(fwhms.len());

    // Compute median and MAD for outlier rejection
    let median = median_f32_mut(&mut fwhms);
    let mad = mad_f32_with_scratch(&fwhms, median, &mut scratch);

    // Reject outliers: keep within 3×MAD of median (with floor for uniform distributions)
    let threshold = FWHM_MAD_MULTIPLIER * mad_floored(mad, median, FWHM_MAD_FLOOR_FRACTION);
    let count_before = fwhms.len();
    fwhms.retain(|&f| (f - median).abs() <= threshold);

    // If too many rejected, use pre-rejection median
    if fwhms.len() < min_stars {
        tracing::debug!(
            "Too many outliers rejected ({count_before} -> {}), using pre-rejection median {median:.2}",
            fwhms.len(),
        );
        // `median` was computed over the pre-rejection set (`count_before` stars),
        // not the shrunken post-retain `fwhms` — report the count that actually
        // produced the returned value.
        return FwhmResult {
            fwhm: Some(median),
            stars_used: count_before,
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

#[cfg(test)]
mod tests;
