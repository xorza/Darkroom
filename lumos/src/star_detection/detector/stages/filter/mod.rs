//! Star filtering stage.
//!
//! Applies quality filters, removes duplicates, and sorts by flux.

use crate::math::statistics::{MedianMad, mad_floored, mad_to_sigma};
use crate::star_detection::config::filter_config::FilterConfig;
use crate::star_detection::detector::QualityFilterDiagnostics;
use crate::star_detection::detector::stages::FWHM_MAD_FLOOR_FRACTION;
use crate::star_detection::star::Star;

/// Result of the filter stage: the surviving stars plus rejection statistics.
#[derive(Debug)]
pub(crate) struct FilterOutcome {
    /// Filtered stars, sorted by flux (brightest first).
    pub(crate) stars: Vec<Star>,
    pub(crate) diagnostics: QualityFilterDiagnostics,
}

/// Why a star fails the quality filter, by the test that caught it, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejection {
    Saturated,
    LowSnr,
    Eccentric,
    CosmicRay,
    NotRound,
}

impl Rejection {
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
            let Some(rejection) = Rejection::of(star, config) else {
                return true;
            };
            *match rejection {
                Rejection::Saturated => &mut diagnostics.saturated,
                Rejection::LowSnr => &mut diagnostics.low_snr,
                Rejection::Eccentric => &mut diagnostics.high_eccentricity,
                Rejection::CosmicRay => &mut diagnostics.cosmic_rays,
                Rejection::NotRound => &mut diagnostics.roundness,
            } += 1;
            false
        });

        // Sort by flux (brightest first)
        sort_by_flux(&mut stars);

        // Filter FWHM outliers
        if let Some(max_deviation) = config.max_fwhm_deviation {
            diagnostics.fwhm_outliers = filter_fwhm_outliers(&mut stars, max_deviation, values);
        }

        // Remove duplicates
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

/// The duplicate search's working sets, kept from frame to frame: each star's cell beside its
/// index, and which stars it keeps.
#[derive(Debug, Default)]
pub(crate) struct DuplicateScratch {
    members: Vec<((i64, i64), usize)>,
    kept: Vec<bool>,
}

impl DuplicateScratch {
    /// Remove duplicate star detections that are too close together.
    ///
    /// For each cluster of stars within `min_separation`, keeps the *first* star
    /// encountered in `stars` and drops the rest: a star is a duplicate of an earlier *kept* star
    /// strictly closer than `min_separation`, so a dropped star never suppresses a later one. Nothing
    /// here compares `.flux`, so callers MUST pass `stars` already sorted by flux descending (as
    /// `FilterOutcome::from_stars` does via `sort_by_flux`) for "first kept" to mean "brightest kept".
    ///
    /// One pass in input order over the stars sorted by their cell of side `min_separation`, each
    /// cell's run in input order: a star reads the earlier stars of its own and the eight neighbouring
    /// cells, found by binary search, so the cost is O(n log n) at any count and the memory one entry
    /// per star, where a dense grid would cost the field's area.
    ///
    /// Deliberately not `registration::spatial::KdTree`, which is the crate's other spatial index:
    /// its radius queries return the stars after a star as well as before it, and the pass needs only
    /// those before.
    pub(crate) fn remove_duplicates(
        &mut self,
        stars: &mut Vec<Star>,
        min_separation: f32,
    ) -> usize {
        // A pair is a duplicate only when strictly closer than `min_separation`, so a separation of
        // zero removes nothing. Said here rather than reached: the cells would divide by it.
        if stars.len() < 2 || min_separation == 0.0 {
            return 0;
        }
        let min_sep_sq = f64::from(min_separation * min_separation);
        let cell_size = f64::from(min_separation);
        let cell_of = |star: &Star| {
            (
                (star.pos.y / cell_size).floor() as i64,
                (star.pos.x / cell_size).floor() as i64,
            )
        };
        let Self { members, kept } = self;
        members.clear();
        members.extend(
            stars
                .iter()
                .enumerate()
                .map(|(index, star)| (cell_of(star), index)),
        );
        members.sort_unstable();
        kept.clear();
        kept.resize(stars.len(), true);
        for i in 0..stars.len() {
            let star = stars[i].pos;
            let (cell_y, cell_x) = cell_of(&stars[i]);
            let duplicate = (-1..=1).any(|dy| {
                (-1..=1).any(|dx| {
                    let cell = (cell_y + dy, cell_x + dx);
                    let start = members.partition_point(|&(member, _)| member < cell);
                    members[start..]
                        .iter()
                        .take_while(|&&(member, j)| member == cell && j < i)
                        .any(|&(_, j)| kept[j] && star.distance_squared(stars[j].pos) < min_sep_sq)
                })
            });
            kept[i] = !duplicate;
        }

        let removed = kept.iter().filter(|&&keep| !keep).count();
        let mut index = 0;
        stars.retain(|_| {
            let keep = kept[index];
            index += 1;
            keep
        });
        removed
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;

#[cfg(test)]
mod tests;
