//! Star filtering stage.
//!
//! Applies quality filters, removes duplicates, and sorts by flux.

use crate::star_detection::config::Config;
use crate::star_detection::star::Star;

/// Statistics from quality filtering (for diagnostics).
#[derive(Debug, Default)]
pub(crate) struct QualityFilterStats {
    pub saturated: usize,
    pub low_snr: usize,
    pub high_eccentricity: usize,
    pub cosmic_rays: usize,
    pub roundness: usize,
    pub fwhm_outliers: usize,
    pub duplicates: usize,
}

/// Filter stars by quality metrics, remove duplicates, and sort by flux.
///
/// Returns the filtered stars and rejection statistics. Stars are returned
/// sorted by flux (brightest first).
pub(crate) fn filter(mut stars: Vec<Star>, config: &Config) -> (Vec<Star>, QualityFilterStats) {
    let mut stats = QualityFilterStats::default();

    // Apply quality filters
    stars.retain(|star| {
        if star.is_saturated() {
            stats.saturated += 1;
            false
        } else if star.snr < config.min_snr {
            stats.low_snr += 1;
            false
        } else if star.eccentricity > config.max_eccentricity {
            stats.high_eccentricity += 1;
            false
        } else if star.is_cosmic_ray(config.max_sharpness) {
            stats.cosmic_rays += 1;
            false
        } else if !star.is_round(config.max_roundness) {
            stats.roundness += 1;
            false
        } else {
            true
        }
    });

    // Sort by flux (brightest first)
    sort_by_flux(&mut stars);

    // Filter FWHM outliers
    stats.fwhm_outliers = filter_fwhm_outliers(&mut stars, config.max_fwhm_deviation);

    // Remove duplicates
    stats.duplicates = remove_duplicate_stars(&mut stars, config.duplicate_min_separation);

    (stars, stats)
}

/// Sort stars by flux (brightest first).
fn sort_by_flux(stars: &mut [Star]) {
    stars.sort_by(|a, b| {
        b.flux
            .partial_cmp(&a.flux)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Filter stars by FWHM using MAD-based outlier detection.
pub(crate) fn filter_fwhm_outliers(stars: &mut Vec<Star>, max_deviation: f32) -> usize {
    if max_deviation <= 0.0 || stars.len() < 5 {
        return 0;
    }

    let reference_count = (stars.len() / 2).max(5).min(stars.len());
    let mut fwhms: Vec<f32> = stars.iter().take(reference_count).map(|s| s.fwhm).collect();
    let (median_fwhm, mad) = crate::math::median_and_mad_f32_mut(&mut fwhms);

    let effective_mad = mad.max(median_fwhm * 0.1);
    let max_fwhm = median_fwhm + max_deviation * effective_mad;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}

/// Remove duplicate star detections that are too close together.
pub(crate) fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    if stars.len() < 2 {
        return 0;
    }

    if stars.len() < 100 {
        return remove_duplicate_stars_simple(stars, min_separation);
    }

    let min_sep_sq = (min_separation * min_separation) as f64;

    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for star in stars.iter() {
        min_x = min_x.min(star.pos.x);
        min_y = min_y.min(star.pos.y);
        max_x = max_x.max(star.pos.x);
        max_y = max_y.max(star.pos.y);
    }

    let cell_size = min_separation as f64;
    let grid_width = ((max_x - min_x) / cell_size).ceil() as usize + 1;
    let grid_height = ((max_y - min_y) / cell_size).ceil() as usize + 1;

    let mut grid: Vec<smallvec::SmallVec<[usize; 4]>> =
        vec![smallvec::SmallVec::new(); grid_width * grid_height];

    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        let star = &stars[i];
        let cell_x = ((star.pos.x - min_x) / cell_size) as usize;
        let cell_y = ((star.pos.y - min_y) / cell_size) as usize;

        let mut is_duplicate = false;
        'outer: for dy in 0..3 {
            let ny = cell_y.wrapping_add(dy).wrapping_sub(1);
            if ny >= grid_height {
                continue;
            }
            for dx in 0..3 {
                let nx = cell_x.wrapping_add(dx).wrapping_sub(1);
                if nx >= grid_width {
                    continue;
                }
                let cell_idx = ny * grid_width + nx;
                for &other_idx in &grid[cell_idx] {
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

        if is_duplicate {
            kept[i] = false;
        } else {
            let cell_idx = cell_y * grid_width + cell_x;
            grid[cell_idx].push(i);
        }
    }

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

/// Simple O(n²) duplicate removal for small star counts.
pub(crate) fn remove_duplicate_stars_simple(stars: &mut Vec<Star>, min_separation: f32) -> usize {
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
