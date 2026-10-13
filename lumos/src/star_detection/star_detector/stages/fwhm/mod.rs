//! FWHM estimation stage.
//!
//! Determines the effective FWHM for matched filtering by either using
//! a manual value, auto-estimating from bright stars, or disabling.

use crate::math::statistics::{mad_floored, mad_with_scratch, median_mut};
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star::Star;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detection_config::extraction_config::ExtractionConfig;
use crate::star_detection::star_detection_config::filter_config::FilterConfig;
use crate::star_detection::star_detection_config::fwhm_config::{FwhmConfig, FwhmMode};
use crate::star_detection::star_detector::detection_result::FwhmSource;
use crate::star_detection::star_detector::stages::FWHM_MAD_FLOOR_FRACTION;
use crate::star_detection::star_detector::stages::detect::{DetectResult, DetectionInput};
use crate::star_detection::star_detector::stages::filter::FilterRejection;
use crate::star_detection::star_detector::stages::prepared_frame::PreparedFrame;

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
    frame: &PreparedFrame,
    config: &StarDetectionConfig,
    pool: &mut DetectionResources,
) -> FwhmSource {
    match config.fwhm.mode {
        None => FwhmSource::Disabled,
        Some(FwhmMode::Fixed(fwhm)) => FwhmSource::Configured(fwhm),
        Some(FwhmMode::Auto { fallback }) => BrightStars {
            frame,
            config,
            fallback,
        }
        .fwhm(pool),
    }
}

/// The FWHM estimate from a frame's bright stars: the frame, its configuration, and `fallback`,
/// the configured seed.
#[derive(Debug, Clone, Copy)]
struct BrightStars<'a> {
    frame: &'a PreparedFrame,
    config: &'a StarDetectionConfig,
    fallback: f32,
}

impl BrightStars<'_> {
    /// Detect bright stars on the detection plane without a matched filter, measure them on the
    /// measurement plane, and estimate the FWHM from them.
    ///
    /// The measurement's stamp radius and the moments' weighting width both scale with the FWHM
    /// they assume, so a star measured at a seed far from its own width comes out biased toward
    /// the seed — a wide star clipped by too small a stamp reads narrow. The first measurement uses
    /// the seed; a second at the radius the first estimate implies corrects for it.
    fn fwhm(self, pool: &mut DetectionResources) -> FwhmSource {
        let Self {
            frame,
            config,
            fallback,
        } = self;
        let first_pass_config = ExtractionConfig {
            sigma_threshold: config.extraction.sigma_threshold
                * config.fwhm.estimation_sigma_factor,
            min_area: 3,
            ..config.extraction.clone()
        };
        let plane = frame.detection_plane(None, config, pool);
        let regions = DetectResult::from_plane(
            DetectionInput {
                plane: &plane,
                no_data: frame.no_data.as_deref(),
            },
            &first_pass_config,
            pool,
        )
        .regions;
        tracing::debug!(
            "FWHM estimation: first pass detected {} bright star candidates",
            regions.len()
        );

        let estimator = Estimator {
            fwhm: &config.fwhm,
            filter: &config.filter,
            fallback,
        };
        let mut at = |seed: f32| {
            let stars = frame.measure_stars(&regions, &config.measurement, Some(seed));
            estimator.estimate(&stars, &mut pool.values, &mut pool.deviations)
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
}

/// What an estimate from measured stars reads: the estimation settings, the quality filter a star
/// passes to count, and the FWHM to fall back to.
#[derive(Debug, Clone, Copy)]
struct Estimator<'a> {
    fwhm: &'a FwhmConfig,
    filter: &'a FilterConfig,
    fallback: f32,
}

impl Estimator<'_> {
    /// Estimate FWHM from a set of detected stars.
    ///
    /// Uses robust statistics (median + MAD) to handle outliers from
    /// cosmic rays, saturated stars, and edge artifacts.
    ///
    /// # Algorithm
    /// 1. Keep the stars that pass the quality filter ([`FilterRejection::of`]) with a plausible
    ///    FWHM
    /// 2. Compute median FWHM from filtered stars
    /// 3. Reject outliers using MAD-based threshold (keep within 3×MAD of median)
    /// 4. Recompute median from remaining stars
    ///
    /// `fwhms` and `deviations` are scratch.
    fn estimate(
        self,
        stars: &[Star],
        fwhms: &mut Vec<f32>,
        deviations: &mut Vec<f32>,
    ) -> FwhmSource {
        let Self {
            fwhm: fwhm_config,
            filter: filter_config,
            fallback,
        } = self;
        let min_stars = fwhm_config.min_stars;

        fwhms.clear();
        fwhms.extend(
            stars
                .iter()
                .filter(|s| {
                    FilterRejection::of(s, filter_config).is_none()
                        && (FWHM_MIN..FWHM_MAX).contains(&s.fwhm)
                })
                .map(|s| s.fwhm),
        );

        if fwhms.len() < min_stars {
            tracing::debug!(
                "Insufficient stars for FWHM estimation: {} < {}, using fallback {:.1}",
                fwhms.len(),
                min_stars,
                fallback
            );
            return FwhmSource::Configured(fallback);
        }

        let median = median_mut(fwhms);
        let mad = mad_with_scratch(fwhms, median, deviations);

        // The floor keeps a uniform set from a zero threshold.
        let threshold = FWHM_MAD_MULTIPLIER * mad_floored(mad, median, FWHM_MAD_FLOOR_FRACTION);
        let count_before = fwhms.len();
        fwhms.retain(|&f| (f - median).abs() <= threshold);

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

        let final_median = median_mut(fwhms);
        tracing::info!(
            "Estimated FWHM: {final_median:.2} pixels from {} stars",
            fwhms.len()
        );

        FwhmSource::Estimated {
            fwhm: final_median,
            stars_used: fwhms.len(),
        }
    }
}

#[cfg(test)]
mod tests;
