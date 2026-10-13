//! Star filtering stage.
//!
//! Applies quality filters, removes duplicates, and sorts by flux.

use crate::math::statistics::median_mad::MedianMad;
use crate::math::statistics::{mad_floored, mad_to_sigma};
use crate::star_detection::star::Star;
use crate::star_detection::star_detection_config::filter_config::FilterConfig;
use crate::star_detection::star_detector::detection_result::QualityFilterDiagnostics;
use crate::star_detection::star_detector::stages::FWHM_MAD_FLOOR_FRACTION;
use crate::star_detection::star_detector::stages::filter::duplicate_scratch::DuplicateScratch;

pub(crate) mod duplicate_scratch;

/// Result of the filter stage: the surviving stars plus rejection statistics.
#[derive(Debug)]
pub(crate) struct FilterOutcome {
    /// Filtered stars, sorted by flux (brightest first).
    pub(crate) stars: Vec<Star>,
    pub(crate) diagnostics: QualityFilterDiagnostics,
}

/// Why a star fails the quality filter, by the test that caught it, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterRejection {
    Saturated,
    LowSnr,
    Eccentric,
    CosmicRay,
    NotRound,
}

impl FilterRejection {
    /// The first quality test `star` fails under `config`, or `None` when it passes them all.
    ///
    /// The one statement of the per-star criteria: the filter stage counts by it, and FWHM
    /// estimation measures only the stars it passes.
    pub(crate) fn of(star: &Star, config: &FilterConfig) -> Option<Self> {
        if star.saturated {
            Some(Self::Saturated)
        } else if star.snr < config.min_snr {
            Some(Self::LowSnr)
        } else if star.eccentricity > config.max_eccentricity {
            Some(Self::Eccentric)
        } else if star.is_cosmic_ray(config.max_sharpness) {
            Some(Self::CosmicRay)
        } else if !star.is_round(config.max_roundness) {
            Some(Self::NotRound)
        } else {
            None
        }
    }
}

impl FilterOutcome {
    /// Filter stars by quality metrics, remove duplicates, and sort by flux, with `values` and
    /// `duplicates` as scratch.
    ///
    /// Returns the filtered stars and rejection statistics. Stars are returned
    /// sorted by flux (brightest first).
    pub(crate) fn from_stars(
        mut stars: Vec<Star>,
        config: &FilterConfig,
        values: &mut Vec<f32>,
        duplicates: &mut DuplicateScratch,
    ) -> Self {
        let mut diagnostics = QualityFilterDiagnostics::default();

        stars.retain(|star| {
            let Some(rejection) = FilterRejection::of(star, config) else {
                return true;
            };
            *match rejection {
                FilterRejection::Saturated => &mut diagnostics.saturated,
                FilterRejection::LowSnr => &mut diagnostics.low_snr,
                FilterRejection::Eccentric => &mut diagnostics.high_eccentricity,
                FilterRejection::CosmicRay => &mut diagnostics.cosmic_rays,
                FilterRejection::NotRound => &mut diagnostics.roundness,
            } += 1;
            false
        });

        sort_by_flux(&mut stars);

        if let Some(max_deviation) = config.max_fwhm_deviation {
            diagnostics.fwhm_outliers = filter_fwhm_outliers(&mut stars, max_deviation, values);
        }

        diagnostics.duplicates =
            duplicates.remove_duplicates(&mut stars, config.duplicate_min_separation);

        Self { stars, diagnostics }
    }
}

/// Sort stars by flux, brightest first, with any NaN flux last.
///
/// A total order: a comparator that calls NaN equal to everything is not transitive, and the
/// standard sorts may panic on one.
fn sort_by_flux(stars: &mut [Star]) {
    stars.sort_by(|a, b| {
        a.flux
            .is_nan()
            .cmp(&b.flux.is_nan())
            .then_with(|| b.flux.total_cmp(&a.flux))
    });
}

/// Filter stars by FWHM using MAD-based outlier detection, with `values` as scratch.
fn filter_fwhm_outliers(stars: &mut Vec<Star>, max_deviation: f32, values: &mut Vec<f32>) -> usize {
    debug_assert!(
        max_deviation > 0.0,
        "validated positive; `None` skips the call"
    );
    if stars.len() < 5 {
        return 0;
    }

    // `stars.len() >= 5` past the early return, so `max(len/2, 5) <= len` — no upper clamp needed.
    let reference_count = (stars.len() / 2).max(5);
    values.clear();
    values.extend(stars.iter().take(reference_count).map(|s| s.fwhm));
    let reference = MedianMad::of_mut(values);

    // In σ, as the option states: `1.4826·MAD` is a normal distribution's σ.
    let sigma = mad_to_sigma(mad_floored(
        reference.mad,
        reference.median,
        FWHM_MAD_FLOOR_FRACTION,
    ));
    let max_fwhm = reference.median + max_deviation * sigma;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
