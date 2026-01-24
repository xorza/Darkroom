//! Tests for star detection.

use super::{Star, StarDetectionConfig, find_stars};
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
