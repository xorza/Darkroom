//! Tests for star detection.

use super::{Star, StarDetectionConfig, compute_fwhm_median_mad, filter_fwhm_outliers, find_stars};
use crate::AstroImage;
use crate::testing::{calibration_dir, init_tracing};

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
    };
    assert!(star.is_saturated());

    let star2 = Star { peak: 0.8, ..star };
    assert!(!star2.is_saturated());
}

#[test]
fn test_star_is_usable() {
    let star = Star {
        x: 10.0,
        y: 10.0,
        flux: 100.0,
        fwhm: 3.0,
        eccentricity: 0.2,
        snr: 50.0,
        peak: 0.8,
    };
    assert!(star.is_usable(10.0, 0.5));

    // Low SNR
    let low_snr = Star { snr: 5.0, ..star };
    assert!(!low_snr.is_usable(10.0, 0.5));

    // Too elongated
    let elongated = Star {
        eccentricity: 0.7,
        ..star
    };
    assert!(!elongated.is_usable(10.0, 0.5));

    // Saturated
    let saturated = Star { peak: 0.98, ..star };
    assert!(!saturated.is_usable(10.0, 0.5));
}

#[test]
fn test_default_config() {
    let config = StarDetectionConfig::default();
    assert_eq!(config.detection_sigma, 4.0);
    assert_eq!(config.min_area, 5);
    assert_eq!(config.background_tile_size, 64);
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_find_stars_on_light_frame() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let lights_dir = cal_dir.join("Lights");
    if !lights_dir.exists() {
        eprintln!("Lights directory not found, skipping test");
        return;
    }

    let files = common::file_utils::astro_image_files(&lights_dir);
    let Some(first_file) = files.first() else {
        eprintln!("No image files in Lights, skipping test");
        return;
    };

    println!("Loading light frame: {:?}", first_file);
    let start = std::time::Instant::now();
    let image = AstroImage::from_file(first_file).expect("Failed to load image");
    println!(
        "Loaded {}x{} image in {:?}",
        image.dimensions.width,
        image.dimensions.height,
        start.elapsed()
    );

    // Convert to grayscale for star detection
    let (width, height) = (image.dimensions.width, image.dimensions.height);
    let grayscale = image.to_grayscale().pixels;

    // Find stars
    let config = StarDetectionConfig::default();
    let start = std::time::Instant::now();
    let stars = find_stars(&grayscale, width, height, &config);
    println!("Found {} stars in {:?}", stars.len(), start.elapsed());

    // Print statistics
    assert!(
        !stars.is_empty(),
        "Should find at least some stars in a light frame"
    );

    // Sort by flux (brightest first)
    let mut stars = stars;
    stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());

    println!("\nTop 10 brightest stars:");
    println!(
        "{:>8} {:>8} {:>10} {:>8} {:>8} {:>8}",
        "X", "Y", "Flux", "FWHM", "SNR", "Ecc"
    );
    for star in stars.iter().take(10) {
        println!(
            "{:>8.2} {:>8.2} {:>10.1} {:>8.2} {:>8.1} {:>8.3}",
            star.x, star.y, star.flux, star.fwhm, star.snr, star.eccentricity
        );
    }

    // Basic sanity checks
    for star in &stars {
        assert!(
            star.x >= 0.0 && star.x < width as f32,
            "Star X out of bounds"
        );
        assert!(
            star.y >= 0.0 && star.y < height as f32,
            "Star Y out of bounds"
        );
        assert!(star.flux > 0.0, "Star flux should be positive");
        assert!(star.fwhm > 0.0, "Star FWHM should be positive");
        assert!(star.snr > 0.0, "Star SNR should be positive");
        assert!(
            star.eccentricity >= 0.0 && star.eccentricity <= 1.0,
            "Eccentricity should be in [0, 1]"
        );
    }

    // Statistics
    let avg_fwhm: f32 = stars.iter().map(|s| s.fwhm).sum::<f32>() / stars.len() as f32;
    let avg_snr: f32 = stars.iter().map(|s| s.snr).sum::<f32>() / stars.len() as f32;
    let avg_ecc: f32 = stars.iter().map(|s| s.eccentricity).sum::<f32>() / stars.len() as f32;

    println!("\nStatistics:");
    println!("  Average FWHM: {:.2} pixels", avg_fwhm);
    println!("  Average SNR: {:.1}", avg_snr);
    println!("  Average eccentricity: {:.3}", avg_ecc);
    println!("  Total stars: {}", stars.len());
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
    }
}

#[test]
fn test_compute_fwhm_median_mad_single_value() {
    let fwhms = vec![3.0];
    let (median, mad) = compute_fwhm_median_mad(&fwhms);

    assert!((median - 3.0).abs() < 1e-6);
    assert!((mad - 0.0).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_odd_count() {
    // [2.0, 3.0, 4.0] -> median = 3.0
    // deviations: [1.0, 0.0, 1.0] -> sorted: [0.0, 1.0, 1.0] -> MAD = 1.0
    let fwhms = vec![2.0, 4.0, 3.0];
    let (median, mad) = compute_fwhm_median_mad(&fwhms);

    assert!((median - 3.0).abs() < 1e-6);
    assert!((mad - 1.0).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_even_count() {
    // [2.0, 3.0, 4.0, 5.0] -> median = fwhms[2] = 4.0 (integer division)
    // deviations from 4.0: [2.0, 1.0, 0.0, 1.0] -> sorted: [0.0, 1.0, 1.0, 2.0] -> MAD = 1.0
    let fwhms = vec![2.0, 3.0, 5.0, 4.0];
    let (median, mad) = compute_fwhm_median_mad(&fwhms);

    assert!((median - 4.0).abs() < 1e-6);
    assert!((mad - 1.0).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_uniform_values() {
    // All same values -> MAD = 0
    let fwhms = vec![3.5, 3.5, 3.5, 3.5, 3.5];
    let (median, mad) = compute_fwhm_median_mad(&fwhms);

    assert!((median - 3.5).abs() < 1e-6);
    assert!((mad - 0.0).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_with_outlier() {
    // [3.0, 3.1, 3.2, 3.0, 10.0] -> sorted: [3.0, 3.0, 3.1, 3.2, 10.0] -> median = 3.1
    // deviations: [0.1, 0.1, 0.0, 0.1, 6.9] -> sorted: [0.0, 0.1, 0.1, 0.1, 6.9] -> MAD = 0.1
    let fwhms = vec![3.0, 3.1, 3.2, 3.0, 10.0];
    let (median, mad) = compute_fwhm_median_mad(&fwhms);

    assert!((median - 3.1).abs() < 1e-6);
    assert!((mad - 0.1).abs() < 1e-6);
}

#[test]
fn test_compute_fwhm_median_mad_large_spread() {
    // [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0] -> median = 4.0
    // deviations: [3.0, 2.0, 1.0, 0.0, 1.0, 2.0, 3.0] -> sorted: [0,1,1,2,2,3,3] -> MAD = 2.0
    let fwhms = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
    let (median, mad) = compute_fwhm_median_mad(&fwhms);

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
