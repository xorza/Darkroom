//! FWHM estimation stage.
//!
//! Determines the effective FWHM for matched filtering by either using
//! a manual value, auto-estimating from bright stars, or disabling.

use crate::common::Buffer2;
use crate::math::{mad_f32_with_scratch, median_f32_mut};

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

/// Minimum MAD as a fraction of median FWHM. Prevents zero-threshold
/// when all FWHM values are near-identical (uniform distribution).
const FWHM_MAD_FLOOR_FRACTION: f32 = 0.1;

use super::detect::detect;
use super::measure;
use crate::star_detection::background::BackgroundEstimate;
use crate::star_detection::buffer_pool::BufferPool;
use crate::star_detection::config::Config;
use crate::star_detection::deblend::Region;
use crate::star_detection::star::Star;

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

    let stars = measure::measure(&regions, pixels, stats, &first_pass_config);

    estimate_fwhm_from_stars(
        &stars,
        config.min_stars_for_fwhm,
        DEFAULT_FWHM,
        config.max_eccentricity,
        config.max_sharpness,
    )
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
#[allow(clippy::needless_pass_by_value)]
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
            !s.is_saturated(0.95)
                && s.eccentricity <= max_eccentricity
                && s.sharpness < max_sharpness
                && (FWHM_MIN..FWHM_MAX).contains(&s.fwhm)
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
    let threshold = FWHM_MAD_MULTIPLIER * mad.max(median * FWHM_MAD_FLOOR_FRACTION);
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec2;

    fn make_star(fwhm: f32, eccentricity: f32, sharpness: f32, peak: f32) -> Star {
        Star {
            pos: DVec2::new(10.0, 10.0),
            flux: 100.0,
            fwhm,
            eccentricity,
            snr: 50.0,
            peak,
            sharpness,
            roundness1: 0.0,
            roundness2: 0.0,
        }
    }

    fn make_good_star(fwhm: f32) -> Star {
        make_star(fwhm, 0.1, 0.3, 0.5)
    }

    // =========================================================================
    // estimate_fwhm_from_stars tests
    // =========================================================================

    #[test]
    fn test_fwhm_estimation_insufficient_stars() {
        // Fewer than min_stars returns default FWHM
        let stars: Vec<Star> = (0..4).map(|_| make_good_star(3.0)).collect();

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        assert!(result.fwhm.is_some());
        assert!((result.fwhm.unwrap() - 4.0).abs() < 0.01); // Default FWHM
        assert_eq!(result.stars_used, 4);
    }

    #[test]
    fn test_fwhm_estimation_filters_saturated() {
        // Saturated stars (peak > 0.95) are excluded
        // 9 good stars at FWHM=3.0 + 1 saturated at FWHM=10.0
        let mut stars: Vec<Star> = (0..10).map(|_| make_good_star(3.0)).collect();
        stars[0] = make_star(10.0, 0.1, 0.3, 0.98); // Saturated with bad FWHM

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        // All 9 good stars have FWHM=3.0, so median should be exactly 3.0
        assert!(result.fwhm.is_some());
        assert!(
            (result.fwhm.unwrap() - 3.0).abs() < 0.01,
            "FWHM {} should be 3.0 (saturated star filtered)",
            result.fwhm.unwrap()
        );
    }

    #[test]
    fn test_fwhm_estimation_filters_high_eccentricity() {
        // High eccentricity stars (> max_eccentricity=0.8) are excluded
        let mut stars: Vec<Star> = (0..10).map(|_| make_good_star(3.0)).collect();
        stars[0] = make_star(10.0, 0.9, 0.3, 0.5); // High eccentricity

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        assert!(result.fwhm.is_some());
        assert!(
            (result.fwhm.unwrap() - 3.0).abs() < 0.01,
            "FWHM {} should be 3.0 (high-ecc star filtered)",
            result.fwhm.unwrap()
        );
    }

    #[test]
    fn test_fwhm_estimation_filters_cosmic_rays() {
        // High sharpness (cosmic rays, sharpness >= 0.7) are excluded
        let mut stars: Vec<Star> = (0..10).map(|_| make_good_star(3.0)).collect();
        stars[0] = make_star(1.0, 0.1, 0.9, 0.5); // Cosmic ray (high sharpness)

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        assert!(result.fwhm.is_some());
        assert!(
            (result.fwhm.unwrap() - 3.0).abs() < 0.01,
            "FWHM {} should be 3.0 (cosmic ray filtered)",
            result.fwhm.unwrap()
        );
    }

    #[test]
    fn test_fwhm_estimation_filters_invalid_fwhm() {
        // FWHM outside valid range (0.5..20.0) are excluded
        let mut stars: Vec<Star> = (0..10).map(|_| make_good_star(3.0)).collect();
        stars[0] = make_good_star(0.2); // Too small
        stars[1] = make_good_star(25.0); // Too large

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        // 8 remaining stars all at FWHM=3.0
        assert!(result.fwhm.is_some());
        assert!(
            (result.fwhm.unwrap() - 3.0).abs() < 0.01,
            "FWHM {} should be 3.0 (invalid FWHM stars filtered)",
            result.fwhm.unwrap()
        );
    }

    #[test]
    fn test_fwhm_estimation_rejects_outliers() {
        // 10 stars at FWHM=3.0 + 2 outliers at 12.0 and 15.0
        let mut stars: Vec<Star> = (0..10).map(|_| make_good_star(3.0)).collect();
        stars.push(make_good_star(12.0));
        stars.push(make_good_star(15.0));

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        // MAD-based rejection should remove the 12.0 and 15.0 outliers
        assert!(result.fwhm.is_some());
        assert!(
            (result.fwhm.unwrap() - 3.0).abs() < 0.01,
            "FWHM {} should be 3.0 (outliers rejected)",
            result.fwhm.unwrap()
        );
    }

    #[test]
    fn test_fwhm_estimation_uniform_values() {
        // All identical FWHM values
        let stars: Vec<Star> = (0..10).map(|_| make_good_star(4.5)).collect();

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        assert!(result.fwhm.is_some());
        assert!((result.fwhm.unwrap() - 4.5).abs() < 0.01);
        assert_eq!(result.stars_used, 10);
    }

    #[test]
    fn test_fwhm_estimation_varying_values() {
        // FWHM values: [2.8, 2.9, 2.9, 3.0, 3.0, 3.0, 3.1, 3.1, 3.2, 3.3]
        // Sorted: median is average of values at indices 4,5 = (3.0+3.0)/2 = 3.0
        // No outliers, so all 10 stars should be used
        let fwhms = [2.8, 3.0, 3.1, 3.2, 2.9, 3.3, 3.0, 3.1, 2.9, 3.0];
        let stars: Vec<Star> = fwhms.iter().map(|&f| make_good_star(f)).collect();

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        assert!(result.fwhm.is_some());
        // Median of sorted [2.8, 2.9, 2.9, 3.0, 3.0, 3.0, 3.1, 3.1, 3.2, 3.3]
        // = value at index 5 = 3.0
        assert!(
            (result.fwhm.unwrap() - 3.0).abs() < 0.05,
            "FWHM {} should be ~3.0 (median of varied values)",
            result.fwhm.unwrap()
        );
        assert_eq!(result.stars_used, 10, "All 10 stars should be used");
    }

    #[test]
    fn test_fwhm_estimation_empty_after_filtering() {
        // All stars filtered out → returns default with 0 stars
        let stars: Vec<Star> = (0..10)
            .map(|_| make_star(3.0, 0.1, 0.3, 0.98)) // All saturated
            .collect();

        let result = estimate_fwhm_from_stars(&stars, 5, 4.0, 0.8, 0.7);

        assert!(result.fwhm.is_some());
        assert!((result.fwhm.unwrap() - 4.0).abs() < 0.01); // Default
        assert_eq!(result.stars_used, 0);
    }

    // =========================================================================
    // FwhmResult tests
    // =========================================================================

    #[test]
    fn test_fwhm_result_debug() {
        let result = FwhmResult {
            fwhm: Some(3.5),
            stars_used: 42,
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("3.5"));
        assert!(debug_str.contains("42"));
    }

    #[test]
    fn test_fwhm_result_clone() {
        let result = FwhmResult {
            fwhm: Some(3.5),
            stars_used: 42,
        };
        let cloned = result;
        assert_eq!(result.fwhm, cloned.fwhm);
        assert_eq!(result.stars_used, cloned.stars_used);
    }
}
