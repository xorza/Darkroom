//! Tests for multi-threshold deblending on dense star fields.
//!
//! These tests require `LUMOS_CALIBRATION_DIR` with dense field images:
//! - `dense1.jpg`, `dense2.jpg`, `dense3.jpg`
//!
//! Run with: `cargo test -p lumos --features real-data dense_field -- --ignored --nocapture`

use crate::AstroImage;
use crate::star_detection::{DeblendConfig, StarDetectionConfig, StarDetector};
use crate::testing::{calibration_dir, init_tracing};
use imaginarium::ColorFormat;

#[test]
#[ignore]
// Requires LUMOS_CALIBRATION_DIR with dense field images
fn test_multi_threshold_on_dense_fields() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let dense_files = [
        //"dense1.jpg", "dense2.jpg", "dense3.jpg",
        "dense4.jpg",
    ];

    for filename in &dense_files {
        let image_path = cal_dir.join(filename);
        if !image_path.exists() {
            eprintln!("{} not found, skipping", filename);
            continue;
        }

        println!("\n============================================================");
        println!("Processing: {}", filename);
        println!("============================================================");

        // Load and convert to grayscale f32
        let img = imaginarium::Image::read_file(&image_path)
            .expect("Failed to load image")
            .packed()
            .convert(ColorFormat::L_F32)
            .expect("Failed to convert to grayscale");

        let astro_image: AstroImage = img.into();
        println!(
            "Image size: {}x{}",
            astro_image.width(),
            astro_image.height()
        );

        // Multi-threshold config (enabled when n_thresholds > 0)
        let config = StarDetectionConfig {
            deblend: DeblendConfig {
                n_thresholds: 32,
                ..Default::default()
            },
            ..StarDetectionConfig::for_crowded_field()
        };

        let mut detector = StarDetector::from_config(config);

        let start = std::time::Instant::now();
        let result = detector.detect(&astro_image);
        let elapsed = start.elapsed();

        println!("Detection time: {:?}", elapsed);
        println!("Stars found: {}", result.stars.len());

        if !result.stars.is_empty() {
            // Statistics
            let avg_fwhm: f32 =
                result.stars.iter().map(|s| s.fwhm).sum::<f32>() / result.stars.len() as f32;
            let avg_snr: f32 =
                result.stars.iter().map(|s| s.snr).sum::<f32>() / result.stars.len() as f32;
            let avg_ecc: f32 = result.stars.iter().map(|s| s.eccentricity).sum::<f32>()
                / result.stars.len() as f32;

            println!("\nStatistics:");
            println!("  Average FWHM: {:.2} px", avg_fwhm);
            println!("  Average SNR: {:.1}", avg_snr);
            println!("  Average eccentricity: {:.3}", avg_ecc);

            // Top 10 brightest
            println!("\nTop 10 brightest stars:");
            println!(
                "{:>8} {:>8} {:>10} {:>8} {:>8}",
                "X", "Y", "Flux", "FWHM", "SNR"
            );
            for star in result.stars.iter().take(10) {
                println!(
                    "{:>8.1} {:>8.1} {:>10.0} {:>8.2} {:>8.1}",
                    star.x, star.y, star.flux, star.fwhm, star.snr
                );
            }
        }

        assert!(
            !result.stars.is_empty(),
            "Should find stars in dense field image {}",
            filename
        );
    }
}

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR with dense field images
fn test_compare_local_maxima_vs_multi_threshold() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let image_path = cal_dir.join("dense1.jpg");
    if !image_path.exists() {
        eprintln!("dense1.jpg not found, skipping test");
        return;
    }

    // Load and convert to grayscale f32
    let img = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed()
        .convert(ColorFormat::L_F32)
        .expect("Failed to convert to grayscale");

    let astro_image: AstroImage = img.into();
    println!(
        "Image size: {}x{}",
        astro_image.width(),
        astro_image.height()
    );

    // Local maxima (simple) - n_thresholds = 0
    let config_local = StarDetectionConfig {
        deblend: DeblendConfig {
            n_thresholds: 0,
            ..Default::default()
        },
        ..StarDetectionConfig::for_crowded_field()
    };

    // Multi-threshold - n_thresholds = 32
    let config_multi = StarDetectionConfig {
        deblend: DeblendConfig {
            n_thresholds: 32,
            ..Default::default()
        },
        ..StarDetectionConfig::for_crowded_field()
    };

    println!("\n--- Local Maxima Deblending ---");
    let mut detector_local = StarDetector::from_config(config_local);
    let start = std::time::Instant::now();
    let result_local = detector_local.detect(&astro_image);
    println!("Time: {:?}", start.elapsed());
    println!("Stars found: {}", result_local.stars.len());

    println!("\n--- Multi-Threshold Deblending ---");
    let mut detector_multi = StarDetector::from_config(config_multi);
    let start = std::time::Instant::now();
    let result_multi = detector_multi.detect(&astro_image);
    println!("Time: {:?}", start.elapsed());
    println!("Stars found: {}", result_multi.stars.len());

    println!("\n--- Comparison ---");
    let diff = result_multi.stars.len() as i32 - result_local.stars.len() as i32;
    println!(
        "Multi-threshold found {} {} stars than local-maxima",
        diff.abs(),
        if diff > 0 { "more" } else { "fewer" }
    );
}
