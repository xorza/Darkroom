//! Star filtering stage.
//!
//! Applies quality filters, removes duplicates, and sorts by flux.

use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star::Star;
use crate::star_detection::star_detection_config::filter_config::FilterConfig;
use crate::star_detection::star_detector::detection_result::QualityFilterDiagnostics;
use crate::star_detection::star_detector::stages::FwhmSpread;

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
    /// Filter stars by quality metrics, remove duplicates, and sort by flux, in the scratch of
    /// `resources`.
    ///
    /// Returns the filtered stars and rejection statistics. Stars are returned
    /// sorted by flux (brightest first).
    pub(crate) fn from_stars(
        mut stars: Vec<Star>,
        config: &FilterConfig,
        resources: &mut DetectionResources,
    ) -> Self {
        let DetectionResources {
            values,
            deviations,
            duplicates,
            ..
        } = resources;
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
            diagnostics.fwhm_outliers = filter_fwhm_outliers(
                &mut stars,
                max_deviation,
                FwhmScratch { values, deviations },
            );
        }

        diagnostics.duplicates = duplicates.remove(&mut stars, config.duplicate_min_separation);

        Self { stars, diagnostics }
    }
}

/// Sort stars by flux, brightest first. A measurement keeps only stars of finite flux.
fn sort_by_flux(stars: &mut [Star]) {
    debug_assert!(stars.iter().all(|star| star.flux.is_finite()));
    stars.sort_by(|a, b| b.flux.total_cmp(&a.flux));
}

/// The scratch an FWHM clip sorts in: the FWHMs, and their deviations.
#[derive(Debug)]
struct FwhmScratch<'a> {
    values: &'a mut Vec<f32>,
    deviations: &'a mut Vec<f32>,
}

/// Remove the stars whose FWHM lies over `max_deviation` σ above the brighter half's median, σ
/// from their MAD ([`FwhmSpread`]).
fn filter_fwhm_outliers(
    stars: &mut Vec<Star>,
    max_deviation: f32,
    scratch: FwhmScratch<'_>,
) -> usize {
    let FwhmScratch { values, deviations } = scratch;
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
    let reference = FwhmSpread::of(values, deviations);
    let max_fwhm = reference.median + max_deviation * reference.sigma;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
