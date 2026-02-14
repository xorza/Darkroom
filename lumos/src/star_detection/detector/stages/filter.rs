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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec2;

    fn make_test_star(fwhm: f32, flux: f32) -> Star {
        Star {
            pos: DVec2::new(10.0, 10.0),
            flux,
            fwhm,
            eccentricity: 0.1,
            snr: 50.0,
            peak: 0.5,
            sharpness: 0.3,
            roundness1: 0.0,
            roundness2: 0.0,
        }
    }

    fn make_star_at(x: f32, y: f32, flux: f32) -> Star {
        Star {
            pos: DVec2::new(x as f64, y as f64),
            flux,
            fwhm: 3.0,
            eccentricity: 0.1,
            snr: 50.0,
            peak: 0.5,
            sharpness: 0.3,
            roundness1: 0.0,
            roundness2: 0.0,
        }
    }

    // =========================================================================
    // FWHM Outlier Filtering Tests
    // =========================================================================

    #[test]
    fn test_filter_fwhm_outliers_disabled_when_zero_deviation() {
        let mut stars: Vec<Star> = (0..10)
            .map(|i| make_test_star(3.0 + i as f32, 100.0 - i as f32))
            .collect();

        let removed = filter_fwhm_outliers(&mut stars, 0.0);

        assert_eq!(removed, 0);
        assert_eq!(stars.len(), 10);
    }

    #[test]
    fn test_filter_fwhm_outliers_disabled_when_too_few_stars() {
        let mut stars: Vec<Star> = (0..4)
            .map(|i| make_test_star(3.0 + i as f32 * 10.0, 100.0 - i as f32))
            .collect();

        let removed = filter_fwhm_outliers(&mut stars, 3.0);

        assert_eq!(removed, 0);
        assert_eq!(stars.len(), 4);
    }

    #[test]
    fn test_filter_fwhm_outliers_removes_single_outlier() {
        // 9 stars with FWHM ~3.0, 1 star with FWHM 20.0
        let mut stars: Vec<Star> = (0..9)
            .map(|i| make_test_star(3.0 + (i as f32 * 0.1), 100.0 - i as f32))
            .collect();
        stars.push(make_test_star(20.0, 10.0)); // Outlier with low flux

        let removed = filter_fwhm_outliers(&mut stars, 3.0);

        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 9);
        assert!(stars.iter().all(|s| s.fwhm < 10.0));
    }

    #[test]
    fn test_filter_fwhm_outliers_removes_multiple_outliers() {
        // 7 stars with FWHM ~3.0, 3 stars with FWHM > 15.0
        let mut stars: Vec<Star> = (0..7)
            .map(|i| make_test_star(3.0 + (i as f32 * 0.1), 100.0 - i as f32))
            .collect();
        stars.push(make_test_star(15.0, 5.0));
        stars.push(make_test_star(18.0, 4.0));
        stars.push(make_test_star(25.0, 3.0));

        let removed = filter_fwhm_outliers(&mut stars, 3.0);

        assert_eq!(removed, 3);
        assert_eq!(stars.len(), 7);
    }

    #[test]
    fn test_filter_fwhm_outliers_keeps_all_when_uniform() {
        // All stars have similar FWHM
        let mut stars: Vec<Star> = (0..10)
            .map(|i| make_test_star(3.0 + (i as f32 * 0.05), 100.0 - i as f32))
            .collect();

        let removed = filter_fwhm_outliers(&mut stars, 3.0);

        assert_eq!(removed, 0);
        assert_eq!(stars.len(), 10);
    }

    #[test]
    fn test_filter_fwhm_outliers_uses_effective_mad_floor() {
        // All identical FWHM values -> MAD = 0, but effective_mad = median * 0.1
        // With median = 3.0, effective_mad = 0.3
        // max_fwhm = 3.0 + 3.0 * 0.3 = 3.9
        let mut stars: Vec<Star> = (0..9)
            .map(|i| make_test_star(3.0, 100.0 - i as f32))
            .collect();
        stars.push(make_test_star(5.0, 10.0)); // Should be removed (5.0 > 3.9)

        let removed = filter_fwhm_outliers(&mut stars, 3.0);

        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 9);
    }

    #[test]
    fn test_filter_fwhm_outliers_uses_top_half_for_reference() {
        // First 5 stars (top half by flux) have FWHM ~3.0
        // Last 5 stars have varying FWHM including outliers
        let mut stars: Vec<Star> = vec![
            make_test_star(3.0, 100.0),
            make_test_star(3.1, 95.0),
            make_test_star(2.9, 90.0),
            make_test_star(3.2, 85.0),
            make_test_star(3.0, 80.0),
            // Lower flux stars - some outliers
            make_test_star(3.5, 50.0),  // Keep
            make_test_star(4.0, 40.0),  // Keep (borderline)
            make_test_star(8.0, 30.0),  // Remove
            make_test_star(3.1, 20.0),  // Keep
            make_test_star(15.0, 10.0), // Remove
        ];

        let removed = filter_fwhm_outliers(&mut stars, 3.0);

        assert!(removed >= 2, "Should remove at least 2 outliers");
        assert!(
            stars.iter().all(|s| s.fwhm < 8.0),
            "All remaining should have FWHM < 8.0"
        );
    }

    #[test]
    fn test_filter_fwhm_outliers_preserves_order() {
        // Stars should remain sorted by flux after filtering
        let mut stars: Vec<Star> = vec![
            make_test_star(3.0, 100.0),
            make_test_star(3.1, 90.0),
            make_test_star(20.0, 80.0), // Outlier
            make_test_star(3.2, 70.0),
            make_test_star(3.0, 60.0),
        ];

        filter_fwhm_outliers(&mut stars, 3.0);

        // Check order is preserved
        for i in 1..stars.len() {
            assert!(
                stars[i - 1].flux >= stars[i].flux,
                "Stars should remain sorted by flux"
            );
        }
    }

    #[test]
    fn test_filter_fwhm_outliers_stricter_deviation() {
        // Stars: FWHM 3.0, 3.2, 3.4, ..., 4.4 (8 stars) + outliers 6.0, 7.0
        // Strict (1.5) should remove more than loose (5.0)
        let mut stars1: Vec<Star> = (0..8)
            .map(|i| make_test_star(3.0 + (i as f32 * 0.2), 100.0 - i as f32))
            .collect();
        stars1.push(make_test_star(6.0, 10.0));
        stars1.push(make_test_star(7.0, 5.0));

        let mut stars2 = stars1.clone();

        let removed_strict = filter_fwhm_outliers(&mut stars1, 1.5);
        let removed_loose = filter_fwhm_outliers(&mut stars2, 5.0);

        // Reference: first 5 stars (FWHM 3.0, 3.2, 3.4, 3.6, 3.8).
        // median = 3.4, MAD = 0.2, effective_mad = max(0.2, 0.34) = 0.34.
        // Strict: max_fwhm = 3.4 + 1.5 * 0.34 = 3.91 → removes 4.0, 4.2, 4.4, 6.0, 7.0 = 5
        // Loose:  max_fwhm = 3.4 + 5.0 * 0.34 = 5.10 → removes 6.0, 7.0 = 2
        assert!(
            removed_strict > removed_loose,
            "Strict ({}) should remove more than loose ({})",
            removed_strict,
            removed_loose
        );
        assert_eq!(
            removed_loose, 2,
            "Loose should remove 2 outliers (6.0, 7.0)"
        );
        assert_eq!(
            removed_strict, 5,
            "Strict should remove 5 stars (FWHM > 3.91)"
        );
    }

    #[test]
    fn test_filter_fwhm_outliers_exactly_five_stars() {
        // Minimum number of stars for filtering to work
        let mut stars: Vec<Star> = vec![
            make_test_star(3.0, 100.0),
            make_test_star(3.1, 90.0),
            make_test_star(3.0, 80.0),
            make_test_star(3.2, 70.0),
            make_test_star(20.0, 60.0), // Outlier
        ];

        let removed = filter_fwhm_outliers(&mut stars, 3.0);

        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 4);
    }

    #[test]
    fn test_filter_fwhm_outliers_negative_deviation_disabled() {
        let mut stars: Vec<Star> = (0..10)
            .map(|i| make_test_star(3.0 + i as f32 * 5.0, 100.0 - i as f32))
            .collect();

        let removed = filter_fwhm_outliers(&mut stars, -1.0);

        assert_eq!(removed, 0);
        assert_eq!(stars.len(), 10);
    }

    // =========================================================================
    // Duplicate Star Removal Tests
    // =========================================================================

    #[test]
    fn test_remove_duplicate_stars_empty() {
        let mut stars: Vec<Star> = vec![];
        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 0);
        assert!(stars.is_empty());
    }

    #[test]
    fn test_remove_duplicate_stars_single() {
        let mut stars = vec![make_star_at(10.0, 10.0, 100.0)];
        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 0);
        assert_eq!(stars.len(), 1);
    }

    #[test]
    fn test_remove_duplicate_stars_no_duplicates() {
        // Stars far apart - no removal
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0),
            make_star_at(50.0, 50.0, 90.0),
            make_star_at(100.0, 100.0, 80.0),
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 0);
        assert_eq!(stars.len(), 3);
    }

    #[test]
    fn test_remove_duplicate_stars_one_pair() {
        // Two stars within separation - keep brighter one
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0), // Brighter - keep
            make_star_at(12.0, 12.0, 90.0),  // Within 8 pixels - remove
            make_star_at(50.0, 50.0, 80.0),  // Far away - keep
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 2);
        assert!((stars[0].pos.x - 10.0).abs() < 0.01);
        assert!((stars[1].pos.x - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_remove_duplicate_stars_keeps_brightest() {
        // Stars sorted by flux (brightest first) - keep first one
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0), // Brightest
            make_star_at(11.0, 11.0, 50.0),  // Dimmer duplicate
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 1);
        assert!((stars[0].flux - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_remove_duplicate_stars_exact_separation() {
        // Stars exactly at separation distance - should NOT be removed
        // Distance = sqrt(6^2 + 6^2) = 8.485 > 8.0
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0),
            make_star_at(16.0, 16.0, 90.0),
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 0);
        assert_eq!(stars.len(), 2);
    }

    #[test]
    fn test_remove_duplicate_stars_just_under_separation() {
        // Stars just under separation - should be removed
        // Distance = sqrt(5^2 + 5^2) = 7.07 < 8.0
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0),
            make_star_at(15.0, 15.0, 90.0),
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 1);
    }

    #[test]
    fn test_remove_duplicate_stars_cluster_of_three() {
        // Three stars in a cluster - keep only brightest
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0), // Keep
            make_star_at(12.0, 10.0, 90.0),  // Remove (close to first)
            make_star_at(14.0, 10.0, 80.0),  // Remove (close to first)
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 2);
        assert_eq!(stars.len(), 1);
        assert!((stars[0].flux - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_remove_duplicate_stars_two_separate_pairs() {
        // Two pairs of duplicates, far apart from each other
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0),  // Pair 1 - keep
            make_star_at(12.0, 10.0, 90.0),   // Pair 1 - remove
            make_star_at(100.0, 100.0, 80.0), // Pair 2 - keep
            make_star_at(102.0, 100.0, 70.0), // Pair 2 - remove
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 2);
        assert_eq!(stars.len(), 2);
        assert!((stars[0].pos.x - 10.0).abs() < 0.01);
        assert!((stars[1].pos.x - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_remove_duplicate_stars_horizontal_line() {
        // Stars in a horizontal line with spacing
        let mut stars = vec![
            make_star_at(0.0, 0.0, 100.0),
            make_star_at(5.0, 0.0, 90.0),  // Within 8 of first
            make_star_at(10.0, 0.0, 80.0), // Within 8 of second (but second removed)
            make_star_at(20.0, 0.0, 70.0), // Far from all remaining
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        // First removes second (5 < 8)
        // First doesn't remove third (10 >= 8)
        // Third is kept, then compared with fourth (distance 10 >= 8)
        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 3);
    }

    #[test]
    fn test_remove_duplicate_stars_vertical_separation() {
        // Stars separated only vertically
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0),
            make_star_at(10.0, 15.0, 90.0), // 5 pixels vertical - remove
            make_star_at(10.0, 25.0, 80.0), // 15 pixels from first - keep
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 2);
    }

    #[test]
    fn test_remove_duplicate_stars_zero_separation() {
        // Zero separation - removes all but one
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0),
            make_star_at(10.0, 10.0, 90.0), // Exact same position
            make_star_at(10.0, 10.0, 80.0), // Exact same position
        ];

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        assert_eq!(removed, 2);
        assert_eq!(stars.len(), 1);
    }

    #[test]
    fn test_remove_duplicate_stars_large_separation_threshold() {
        // Large separation threshold removes more
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0),
            make_star_at(30.0, 10.0, 90.0), // 20 pixels away
            make_star_at(50.0, 10.0, 80.0), // 40 pixels from first
        ];

        let removed = remove_duplicate_stars(&mut stars, 25.0);

        // 20 < 25, so second is removed
        // 40 >= 25, so third is kept
        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 2);
    }

    #[test]
    fn test_remove_duplicate_stars_preserves_order() {
        // Remaining stars should maintain their relative order
        let mut stars = vec![
            make_star_at(10.0, 10.0, 100.0),
            make_star_at(12.0, 10.0, 95.0), // Remove
            make_star_at(50.0, 50.0, 90.0),
            make_star_at(100.0, 100.0, 85.0),
        ];

        remove_duplicate_stars(&mut stars, 8.0);

        // Check order is preserved
        assert!(stars[0].flux > stars[1].flux);
        assert!(stars[1].flux > stars[2].flux);
    }

    #[test]
    fn test_remove_duplicate_stars_many_duplicates() {
        // 20 stars along x=10..19.5, y=10, spacing=0.5px, all within 8px of star[0]
        // Star[0] at x=10 has highest flux (100), so it survives.
        // Stars at x=10.5..19.5 are within 9.5px of star[0].
        // All stars within 8px of any brighter star get removed.
        // Star at x=18.0 is 8.0px from star[0] — at boundary (not removed since
        // distance must be strictly less). But star at x=17.5 is 7.5 < 8.0 → removed.
        let mut stars: Vec<Star> = (0..20)
            .map(|i| make_star_at(10.0 + (i as f32 * 0.5), 10.0, 100.0 - i as f32))
            .collect();

        let removed = remove_duplicate_stars(&mut stars, 8.0);

        // Star[0] (x=10.0, flux=100): kept.
        // Stars[1..16] (x=10.5..17.5): dist < 8.0 from star[0] → removed (15 stars).
        // Star[16] (x=18.0, flux=84): dist = 8.0 from star[0]. 8^2 = 64 is NOT < 64 → kept.
        // Stars[17..20] (x=18.5..19.5): dist < 8.0 from star[16] → removed (3 stars).
        // Total: 15 + 3 = 18 removed, 2 survivors.
        assert_eq!(
            removed, 18,
            "Should remove 18 of 20 clustered stars, removed {}",
            removed
        );
        assert_eq!(stars.len(), 2, "Star[0] and star[16] should survive");
    }

    #[test]
    fn test_remove_duplicate_stars_spatial_hash_path() {
        // Test with >100 stars to exercise spatial hashing code path
        // Create a grid of stars with some duplicates
        let mut stars: Vec<Star> = Vec::new();

        // Create 150 stars in a grid pattern (15x10)
        for y in 0..10 {
            for x in 0..15 {
                let px = x as f32 * 20.0 + 10.0; // 20 pixel spacing
                let py = y as f32 * 20.0 + 10.0;
                let flux = 1000.0 - (y * 15 + x) as f32; // Decreasing flux
                stars.push(make_star_at(px, py, flux));
            }
        }

        // Add some duplicates close to existing stars
        stars.push(make_star_at(12.0, 12.0, 50.0)); // Close to (10, 10)
        stars.push(make_star_at(32.0, 12.0, 45.0)); // Close to (30, 10)
        stars.push(make_star_at(52.0, 32.0, 40.0)); // Close to (50, 30)

        // Sort by flux (required)
        stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());

        let initial_count = stars.len();
        let removed = remove_duplicate_stars(&mut stars, 8.0);

        // Should remove the 3 duplicates
        assert_eq!(removed, 3);
        assert_eq!(stars.len(), initial_count - 3);

        // Verify no remaining stars are too close
        for i in 0..stars.len() {
            for j in (i + 1)..stars.len() {
                let dx = stars[i].pos.x - stars[j].pos.x;
                let dy = stars[i].pos.y - stars[j].pos.y;
                let dist_sq = dx * dx + dy * dy;
                assert!(
                    dist_sq >= 64.0, // 8.0^2
                    "Stars at ({}, {}) and ({}, {}) are too close: dist={}",
                    stars[i].pos.x,
                    stars[i].pos.y,
                    stars[j].pos.x,
                    stars[j].pos.y,
                    dist_sq.sqrt()
                );
            }
        }
    }

    #[test]
    fn test_remove_duplicate_stars_spatial_hash_edge_cases() {
        // Test edge cases for spatial hashing: stars at grid cell boundaries
        let mut stars: Vec<Star> = Vec::new();

        // Create 200 stars spread across a large area
        for i in 0..200 {
            let x = (i % 20) as f32 * 100.0 + 50.0;
            let y = (i / 20) as f32 * 100.0 + 50.0;
            stars.push(make_star_at(x, y, 1000.0 - i as f32));
        }

        // Add duplicates at cell boundaries (separation = 5.0, so cell size = 5.0)
        // Star at boundary between cells
        stars.push(make_star_at(52.0, 50.0, 10.0)); // Close to (50, 50)

        stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());

        let removed = remove_duplicate_stars(&mut stars, 5.0);

        assert_eq!(removed, 1);
        assert_eq!(stars.len(), 200);
    }

    #[test]
    fn test_remove_duplicate_stars_spatial_hash_consistency() {
        // Verify spatial hash gives same results as simple algorithm
        use rand::prelude::*;

        let mut rng = StdRng::seed_from_u64(12345);

        // Generate 500 random stars
        let base_stars: Vec<Star> = (0..500)
            .map(|i| {
                make_star_at(
                    rng.random_range(0.0..1000.0),
                    rng.random_range(0.0..1000.0),
                    1000.0 - i as f32,
                )
            })
            .collect();

        // Run with spatial hash (>100 stars)
        let mut stars_hash = base_stars.clone();
        stars_hash.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());
        let removed_hash = remove_duplicate_stars(&mut stars_hash, 10.0);

        // Run with simple algorithm (force by using small chunks)
        let mut stars_simple = base_stars;
        stars_simple.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());
        let removed_simple = remove_duplicate_stars_simple(&mut stars_simple, 10.0);

        // Results should match
        assert_eq!(
            removed_hash, removed_simple,
            "Spatial hash removed {} but simple removed {}",
            removed_hash, removed_simple
        );
        assert_eq!(stars_hash.len(), stars_simple.len());

        // Verify same stars kept (by position)
        for (h, s) in stars_hash.iter().zip(stars_simple.iter()) {
            assert!(
                (h.pos.x - s.pos.x).abs() < 0.001 && (h.pos.y - s.pos.y).abs() < 0.001,
                "Mismatch: hash({}, {}) vs simple({}, {})",
                h.pos.x,
                h.pos.y,
                s.pos.x,
                s.pos.y
            );
        }
    }
}
