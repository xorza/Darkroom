//! Star filtering stage.
//!
//! Applies quality filters, removes duplicates, and sorts by flux.

use std::cmp::Ordering;
use std::collections::HashMap;

use smallvec::SmallVec;

use crate::math::statistics::{mad_floored, median_and_mad_f32_mut};
use crate::stacking::star_detection::config::FilterConfig;
use crate::stacking::star_detection::detector::QualityFilterDiagnostics;
use crate::stacking::star_detection::detector::stages::FWHM_MAD_FLOOR_FRACTION;
use crate::stacking::star_detection::star::{SATURATION_PEAK, Star};

/// Below this star count, dedup with the O(n²) brute force instead of the sparse spatial hash.
/// For a handful of stars the brute force is trivial and skips the hash's per-call allocation; the
/// crossover is conservative — the spatial hash is O(stars) and competitive well below this.
const SPATIAL_HASH_CROSSOVER: usize = 100;

/// Result of the filter stage: the surviving stars plus rejection statistics.
#[derive(Debug)]
pub(crate) struct FilterOutcome {
    /// Filtered stars, sorted by flux (brightest first).
    pub(crate) stars: Vec<Star>,
    pub(crate) diagnostics: QualityFilterDiagnostics,
}

/// Filter stars by quality metrics, remove duplicates, and sort by flux.
///
/// Returns the filtered stars and rejection statistics. Stars are returned
/// sorted by flux (brightest first).
pub(crate) fn filter(mut stars: Vec<Star>, config: &FilterConfig) -> FilterOutcome {
    let mut diagnostics = QualityFilterDiagnostics::default();

    // Apply quality filters
    stars.retain(|star| {
        if star.is_saturated(SATURATION_PEAK) {
            diagnostics.saturated += 1;
            false
        } else if star.snr < config.min_snr {
            diagnostics.low_snr += 1;
            false
        } else if star.eccentricity > config.max_eccentricity {
            diagnostics.high_eccentricity += 1;
            false
        } else if star.is_cosmic_ray(config.max_sharpness) {
            diagnostics.cosmic_rays += 1;
            false
        } else if !star.is_round(config.max_roundness) {
            diagnostics.roundness += 1;
            false
        } else {
            true
        }
    });

    // Sort by flux (brightest first)
    sort_by_flux(&mut stars);

    // Filter FWHM outliers
    diagnostics.fwhm_outliers = filter_fwhm_outliers(&mut stars, config.max_fwhm_deviation);

    // Remove duplicates
    diagnostics.duplicates = remove_duplicate_stars(&mut stars, config.duplicate_min_separation);

    FilterOutcome { stars, diagnostics }
}

/// Sort stars by flux (brightest first).
fn sort_by_flux(stars: &mut [Star]) {
    stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap_or(Ordering::Equal));
}

/// Filter stars by FWHM using MAD-based outlier detection.
fn filter_fwhm_outliers(stars: &mut Vec<Star>, max_deviation: f32) -> usize {
    if max_deviation <= 0.0 || stars.len() < 5 {
        return 0;
    }

    // `stars.len() >= 5` past the early return, so `max(len/2, 5) <= len` — no upper clamp needed.
    let reference_count = (stars.len() / 2).max(5);
    let mut fwhms: Vec<f32> = stars.iter().take(reference_count).map(|s| s.fwhm).collect();
    let reference = median_and_mad_f32_mut(&mut fwhms);

    let effective_mad = mad_floored(reference.mad, reference.median, FWHM_MAD_FLOOR_FRACTION);
    let max_fwhm = reference.median + max_deviation * effective_mad;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}

/// Remove duplicate star detections that are too close together.
///
/// For each cluster of stars within `min_separation`, keeps the *first* star
/// encountered in `stars` and drops the rest — neither this function nor its
/// `_simple`/spatial-hash helpers ever compare `.flux`. Callers therefore MUST
/// pass `stars` already sorted by flux descending (as `filter()` does via
/// `sort_by_flux` before calling this) for "first kept" to mean "brightest
/// kept"; otherwise an arbitrary, non-brightest star in each cluster survives.
///
/// Deliberately not `registration::spatial::KdTree`, which is the crate's other spatial index.
/// That one is built once over a fixed point set; this queries a set that *grows as it decides* —
/// only stars already kept are in the grid, which is what makes "first kept wins" hold. The same
/// answer can be had from a static tree over every star plus a `neighbour < i && kept[neighbour]`
/// filter, at the cost of an O(n log n) build and n radius queries whose results are mostly
/// discarded, in place of a structure built as the single pass goes.
fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    if stars.len() < 2 {
        return 0;
    }

    if stars.len() < SPATIAL_HASH_CROSSOVER {
        return remove_duplicate_stars_simple(stars, min_separation);
    }

    let min_sep_sq = (min_separation * min_separation) as f64;
    let cell_size = min_separation as f64;

    // Sparse spatial hash keyed by integer cell coordinate: only cells that actually hold a star
    // are allocated, so memory and time are O(stars). A dense grid is O(field_area / min_sep²) —
    // a 6k×6k field with a few thousand stars otherwise allocates (and zeroes) millions of empty
    // cells, which dominated the cost. A star is a duplicate of an earlier *kept* star within
    // `min_separation`; the grid only ever holds kept stars, so this matches the old behaviour.
    let mut grid: HashMap<(i64, i64), SmallVec<[usize; 4]>> = HashMap::new();
    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        let star = &stars[i];
        let cell_x = (star.pos.x / cell_size).floor() as i64;
        let cell_y = (star.pos.y / cell_size).floor() as i64;

        let mut is_duplicate = false;
        'outer: for dy in -1..=1 {
            for dx in -1..=1 {
                if let Some(cell) = grid.get(&(cell_x + dx, cell_y + dy)) {
                    for &other_idx in cell {
                        let other = &stars[other_idx];
                        let ddx = star.pos.x - other.pos.x;
                        let ddy = star.pos.y - other.pos.y;
                        if ddx * ddx + ddy * ddy < min_sep_sq {
                            is_duplicate = true;
                            break 'outer;
                        }
                    }
                }
            }
        }

        if is_duplicate {
            kept[i] = false;
        } else {
            grid.entry((cell_x, cell_y)).or_default().push(i);
        }
    }

    compact_by_mask(stars, &kept)
}

/// Simple O(n²) duplicate removal for small star counts.
fn remove_duplicate_stars_simple(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    let min_sep_sq = (min_separation * min_separation) as f64;
    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        if !kept[i] {
            continue;
        }
        for j in (i + 1)..stars.len() {
            if !kept[j] {
                continue;
            }
            let dx = stars[i].pos.x - stars[j].pos.x;
            let dy = stars[i].pos.y - stars[j].pos.y;
            if dx * dx + dy * dy < min_sep_sq {
                kept[j] = false;
            }
        }
    }

    compact_by_mask(stars, &kept)
}

/// In-place compaction: remove stars where `kept[i]` is false. Returns removed count.
fn compact_by_mask(stars: &mut Vec<Star>, kept: &[bool]) -> usize {
    let removed_count = kept.iter().filter(|&&k| !k).count();

    let mut write_idx = 0;
    for read_idx in 0..stars.len() {
        if kept[read_idx] {
            if write_idx != read_idx {
                stars[write_idx] = stars[read_idx];
            }
            write_idx += 1;
        }
    }
    stars.truncate(write_idx);

    removed_count
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::stacking::star_detection::star::Star;

    /// Exposes `remove_duplicate_stars` to the detector's benchmarks; production
    /// code only ever reaches it through `filter()`.
    pub(crate) fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
        super::remove_duplicate_stars(stars, min_separation)
    }
}

#[cfg(test)]
mod tests;
