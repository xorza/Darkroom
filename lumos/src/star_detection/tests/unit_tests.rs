//! Unit tests for star detection.

use crate::star_detection::{
    Star, StarDetectionConfig, compute_fwhm_median_mad, filter_fwhm_outliers,
    remove_duplicate_stars,
};

#[test]
fn test_star_is_saturated() {
    let star = Star {
        x: 10.0,
        y: 10.0,
        flux: 100.0,
        fwhm: 3.0,
        eccentricity: 0.1,
        snr: 50.0,
        peak: 0.96,
        sharpness: 0.3,
        roundness1: 0.0,
        roundness2: 0.0,
        laplacian_snr: 0.0,
    };
    assert!(star.is_saturated());

    let star2 = Star { peak: 0.8, ..star };
    assert!(!star2.is_saturated());
}

#[test]
fn test_star_passes_quality_filters() {
    let star = Star {
        x: 10.0,
        y: 10.0,
        flux: 100.0,
        fwhm: 3.0,
        eccentricity: 0.2,
        snr: 50.0,
        peak: 0.8,
        sharpness: 0.3,
        roundness1: 0.0,
        roundness2: 0.0,
        laplacian_snr: 0.0,
    };
    assert!(star.passes_quality_filters(10.0, 0.5, 0.7, 1.0));

    // Low SNR
    let low_snr = Star { snr: 5.0, ..star };
    assert!(!low_snr.passes_quality_filters(10.0, 0.5, 0.7, 1.0));

    // Too elongated
    let elongated = Star {
        eccentricity: 0.7,
        ..star
    };
    assert!(!elongated.passes_quality_filters(10.0, 0.5, 0.7, 1.0));

    // Saturated
    let saturated = Star { peak: 0.98, ..star };
    assert!(!saturated.passes_quality_filters(10.0, 0.5, 0.7, 1.0));

    // Cosmic ray (too sharp)
    let cosmic_ray = Star {
        sharpness: 0.9,
        ..star
    };
    assert!(!cosmic_ray.passes_quality_filters(10.0, 0.5, 0.7, 1.0));

    // Non-round (fails roundness check)
    let non_round = Star {
        roundness1: 0.5,
        ..star
    };
    assert!(!non_round.passes_quality_filters(10.0, 0.5, 0.7, 0.3));
}

#[test]
fn test_default_config() {
    let config = StarDetectionConfig::default();
    assert_eq!(config.detection_sigma, 4.0);
    assert_eq!(config.min_area, 5);
    assert_eq!(config.background_tile_size, 64);
}

// =============================================================================
// FWHM Median/MAD Computation Tests
// =============================================================================

fn make_test_star(fwhm: f32, flux: f32) -> Star {
    Star {
        x: 10.0,
        y: 10.0,
        flux,
        fwhm,
        eccentricity: 0.1,
        snr: 50.0,
        peak: 0.5,
        sharpness: 0.3,
        roundness1: 0.0,
        roundness2: 0.0,
        laplacian_snr: 0.0,
    }
}

#[test]
fn test_compute_fwhm_median_mad_single_value() {
    let fwhms = vec![3.0];
    let (median, mad) = compute_fwhm_median_mad(fwhms);

    assert!((median - 3.0).abs() < 1e-6);
    assert!((mad - 0.0).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_odd_count() {
    // [2.0, 3.0, 4.0] -> median = 3.0
    // deviations: [1.0, 0.0, 1.0] -> sorted: [0.0, 1.0, 1.0] -> MAD = 1.0
    let fwhms = vec![2.0, 4.0, 3.0];
    let (median, mad) = compute_fwhm_median_mad(fwhms);

    assert!((median - 3.0).abs() < 1e-6);
    assert!((mad - 1.0).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_even_count() {
    // [2.0, 3.0, 4.0, 5.0] -> median = fwhms[2] = 4.0 (integer division)
    // deviations from 4.0: [2.0, 1.0, 0.0, 1.0] -> sorted: [0.0, 1.0, 1.0, 2.0] -> MAD = 1.0
    let fwhms = vec![2.0, 3.0, 5.0, 4.0];
    let (median, mad) = compute_fwhm_median_mad(fwhms);

    assert!((median - 4.0).abs() < 1e-6);
    assert!((mad - 1.0).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_uniform_values() {
    // All same values -> MAD = 0
    let fwhms = vec![3.5, 3.5, 3.5, 3.5, 3.5];
    let (median, mad) = compute_fwhm_median_mad(fwhms);

    assert!((median - 3.5).abs() < 1e-6);
    assert!((mad - 0.0).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_with_outlier() {
    // [3.0, 3.1, 3.2, 3.0, 10.0] -> sorted: [3.0, 3.0, 3.1, 3.2, 10.0] -> median = 3.1
    // deviations: [0.1, 0.1, 0.0, 0.1, 6.9] -> sorted: [0.0, 0.1, 0.1, 0.1, 6.9] -> MAD = 0.1
    let fwhms = vec![3.0, 3.1, 3.2, 3.0, 10.0];
    let (median, mad) = compute_fwhm_median_mad(fwhms);

    assert!((median - 3.1).abs() < 1e-6);
    assert!((mad - 0.1).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_large_spread() {
    // [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0] -> median = 4.0
    // deviations: [3.0, 2.0, 1.0, 0.0, 1.0, 2.0, 3.0] -> sorted: [0,1,1,2,2,3,3] -> MAD = 2.0
    let fwhms = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
    let (median, mad) = compute_fwhm_median_mad(fwhms);

    assert!((median - 4.0).abs() < 1e-6);
    assert!((mad - 2.0).abs() < 1e-6);
}

// =============================================================================
// FWHM Outlier Filtering Tests
// =============================================================================

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
    // With stricter deviation (1.5 instead of 3.0), more stars are removed
    let mut stars1: Vec<Star> = (0..8)
        .map(|i| make_test_star(3.0 + (i as f32 * 0.2), 100.0 - i as f32))
        .collect();
    stars1.push(make_test_star(6.0, 10.0));
    stars1.push(make_test_star(7.0, 5.0));

    let mut stars2 = stars1.clone();

    let removed_strict = filter_fwhm_outliers(&mut stars1, 1.5);
    let removed_loose = filter_fwhm_outliers(&mut stars2, 5.0);

    assert!(
        removed_strict >= removed_loose,
        "Stricter deviation should remove at least as many stars"
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

// =============================================================================
// Duplicate Star Removal Tests
// =============================================================================

fn make_star_at(x: f32, y: f32, flux: f32) -> Star {
    Star {
        x,
        y,
        flux,
        fwhm: 3.0,
        eccentricity: 0.1,
        snr: 50.0,
        peak: 0.5,
        sharpness: 0.3,
        roundness1: 0.0,
        roundness2: 0.0,
        laplacian_snr: 0.0,
    }
}

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
    assert!((stars[0].x - 10.0).abs() < 0.01);
    assert!((stars[1].x - 50.0).abs() < 0.01);
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
    assert!((stars[0].x - 10.0).abs() < 0.01);
    assert!((stars[1].x - 100.0).abs() < 0.01);
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
    // Many stars clustered around one point
    let mut stars: Vec<Star> = (0..20)
        .map(|i| make_star_at(10.0 + (i as f32 * 0.5), 10.0, 100.0 - i as f32))
        .collect();

    let removed = remove_duplicate_stars(&mut stars, 8.0);

    // Many should be removed since they're within 8 pixels of each other
    assert!(removed > 10);
    assert!(stars.len() < 10);
}
