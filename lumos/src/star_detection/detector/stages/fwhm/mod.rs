//! FWHM estimation stage.
//!
//! Determines the effective FWHM for matched filtering by either using
//! a manual value, auto-estimating from bright stars, or disabling.

use crate::bit_buffer2::BitBuffer2;
use crate::math::statistics::{mad_floored, mad_with_scratch, median_mut};
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::config::Config;
use crate::star_detection::config::detection_config::DetectionConfig;
use crate::star_detection::config::filter_config::FilterConfig;
use crate::star_detection::config::fwhm_config::{FwhmConfig, FwhmMode};
use crate::star_detection::detector::FwhmSource;
use crate::star_detection::detector::stages::FWHM_MAD_FLOOR_FRACTION;
use crate::star_detection::detector::stages::detect::DetectResult;
use crate::star_detection::detector::stages::filter::Rejection;
use crate::star_detection::detector::stages::measure;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star::Star;
use imaginarium::Buffer2;

/// Minimum plausible FWHM in pixels. Stars narrower than this are likely
/// cosmic rays or hot pixels.
const FWHM_MIN: f32 = 0.5;

/// Maximum plausible FWHM in pixels. Sources broader than this are likely
/// galaxies, nebulae, or artifacts rather than point sources.
const FWHM_MAX: f32 = 20.0;

/// MAD multiplier for outlier rejection in FWHM estimation.
/// Stars with FWHM deviating more than this many MADs from the median are rejected.
const FWHM_MAD_MULTIPLIER: f32 = 3.0;

/// Determine the effective FWHM for matched filtering, and where it came from.
///
/// A [`FwhmSource::Configured`] answer is one this frame did not measure — a fixed FWHM, or an
/// estimate's fallback when too few stars passed — and [`FwhmSource::Estimated`] carries the
/// count of stars that actually produced the value.
pub(crate) fn estimate(
    residual: &Buffer2<f32>,
    sky: &SkyNoise,
    saturation: &BitBuffer2,
    config: &Config,
    pool: &mut DetectionResources,
) -> FwhmSource {
    match config.fwhm.mode {
        None => FwhmSource::Disabled,
        Some(FwhmMode::Fixed(fwhm)) => FwhmSource::Configured(fwhm),
        Some(FwhmMode::Auto { fallback }) => {
            from_bright_stars(residual, sky, saturation, config, fallback, pool)
        }
    }
}

/// Detect bright stars without a matched filter and estimate the FWHM from them.
///
/// The measurement's stamp radius and the moments' weighting width both scale with the FWHM they
/// assume, so a star measured at a seed far from its own width comes out biased toward the seed —
/// a wide star clipped by too small a stamp reads narrow. The first measurement uses `fallback`,
/// the configured seed; a second at the radius the first estimate implies corrects for it.
fn from_bright_stars(
    residual: &Buffer2<f32>,
    sky: &SkyNoise,
    saturation: &BitBuffer2,
    config: &Config,
    fallback: f32,
    pool: &mut DetectionResources,
) -> FwhmSource {
    let first_pass_config = DetectionConfig {
        sigma_threshold: config.detection.sigma_threshold * config.fwhm.estimation_sigma_factor,
        min_area: 3,
        ..config.detection.clone()
    };
    let regions = DetectResult::from_image(residual, sky, None, &first_pass_config, pool).regions;
    tracing::debug!(
        "FWHM estimation: first pass detected {} bright star candidates",
        regions.len()
    );

    let at = |seed: f32| {
        let stars = measure::measure(
            &regions,
            residual,
            sky,
            saturation,
            &config.measurement,
            Some(seed),
        );
        from_stars(&stars, &config.fwhm, fallback, &config.filter)
    };
    let first = at(fallback);
    let FwhmSource::Estimated { fwhm, .. } = first else {
        return first;
    };
    match at(fwhm) {
        second @ FwhmSource::Estimated { .. } => second,
        FwhmSource::Configured(_) | FwhmSource::Disabled => first,
    }
}

/// Estimate FWHM from a set of detected stars.
///
/// Uses robust statistics (median + MAD) to handle outliers from
/// cosmic rays, saturated stars, and edge artifacts.
///
/// # Algorithm
/// 1. Keep the stars that pass the quality filter ([`Rejection::of`]) with a plausible FWHM
/// 2. Compute median FWHM from filtered stars
/// 3. Reject outliers using MAD-based threshold (keep within 3×MAD of median)
/// 4. Recompute median from remaining stars
fn from_stars(
    stars: &[Star],
    fwhm_config: &FwhmConfig,
    fallback: f32,
    filter_config: &FilterConfig,
) -> FwhmSource {
    let min_stars = fwhm_config.min_stars;

    // Filter stars for quality and collect FWHM values
    let mut fwhms: Vec<f32> = stars
        .iter()
        .filter(|s| {
            Rejection::of(s, filter_config).is_none() && (FWHM_MIN..FWHM_MAX).contains(&s.fwhm)
        })
        .map(|s| s.fwhm)
        .collect();

    if fwhms.len() < min_stars {
        tracing::debug!(
            "Insufficient stars for FWHM estimation: {} < {}, using fallback {:.1}",
            fwhms.len(),
            min_stars,
            fallback
        );
        return FwhmSource::Configured(fallback);
    }

    // Scratch buffer for MAD computation
    let mut scratch = Vec::with_capacity(fwhms.len());

    // Compute median and MAD for outlier rejection
    let median = median_mut(&mut fwhms);
    let mad = mad_with_scratch(&fwhms, median, &mut scratch);

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
        // `median` was computed over the pre-rejection set (`count_before` stars), not the
        // shrunken post-retain `fwhms` — report the count that actually produced the value.
        return FwhmSource::Estimated {
            fwhm: median,
            stars_used: count_before,
        };
    }

    let final_median = median_mut(&mut fwhms);
    tracing::info!(
        "Estimated FWHM: {final_median:.2} pixels from {} stars",
        fwhms.len()
    );

    FwhmSource::Estimated {
        fwhm: final_median,
        stars_used: fwhms.len(),
    }
}

#[cfg(test)]
mod tests;
