//! Test star detection on rho-opiuchi.jpg real image.
//!
//! Run with: `cargo test -p lumos --features real-data rho_opiuchi -- --ignored --nocapture`

use crate::star_detection::StarDetector;
use crate::star_detection::config::StarDetectionConfig;
use crate::testing::{calibration_dir, init_tracing};
use crate::{AstroImage, CentroidMethod};
use common::test_utils::test_output_path;
use glam::Vec2;
use imaginarium::Color;
use imaginarium::ColorFormat;
use imaginarium::drawing::draw_circle;

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_detect_rho_opiuchi() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let image_path = cal_dir.join("rho-opiuchi.jpg");
    if !image_path.exists() {
        panic!("rho-opiuchi.jpg not found in {:?}", cal_dir);
    }

    println!("Loading: {:?}", image_path);

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

    let mut detector = StarDetector::from_config(StarDetectionConfig::default().precise_ground());

    let start = std::time::Instant::now();
    let result = detector.detect(&astro_image);
    let elapsed = start.elapsed();

    println!("Detection time: {:?}", elapsed);
    println!("Stars found: {}", result.stars.len());

    if !result.stars.is_empty() {
        let avg_fwhm: f32 =
            result.stars.iter().map(|s| s.fwhm).sum::<f32>() / result.stars.len() as f32;
        let avg_snr: f32 =
            result.stars.iter().map(|s| s.snr).sum::<f32>() / result.stars.len() as f32;

        println!("\nStatistics:");
        println!("  Average FWHM: {:.2} px", avg_fwhm);
        println!("  Average SNR: {:.1}", avg_snr);

        println!("\nTop 10 brightest stars:");
        println!(
            "{:>8} {:>8} {:>10} {:>8} {:>8}",
            "X", "Y", "Flux", "FWHM", "SNR"
        );
        for star in result.stars.iter().take(10) {
            println!(
                "{:>8.1} {:>8.1} {:>10.0} {:>8.2} {:>8.1}",
                star.pos.x, star.pos.y, star.flux, star.fwhm, star.snr
            );
        }
    }

    // Load original image for visualization (RGB_F32 for drawing functions)
    let mut output_img = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed()
        .convert(ColorFormat::RGB_F32)
        .expect("Failed to convert to RGB_F32");

    // Draw circles around all detected stars
    for star in result.stars.iter() {
        let radius = (star.fwhm * 1.5).max(3.0);
        draw_circle(
            &mut output_img,
            Vec2::new(star.pos.x as f32, star.pos.y as f32),
            radius,
            Color::GREEN,
            1.0,
        );
    }
    println!("Drew {} circles", result.stars.len());

    // Convert back to RGB_U8 for saving
    let output_img = output_img
        .convert(ColorFormat::RGB_U8)
        .expect("Failed to convert to RGB_U8");

    // Save output
    let output_path = test_output_path("rho-opiuchi-detection.jpg");
    output_img
        .save_file(&output_path)
        .expect("Failed to save output image");
    println!("\nSaved detection result to: {:?}", output_path);

    assert!(
        !result.stars.is_empty(),
        "Should find stars in rho-opiuchi.jpg"
    );
}

#[bench::quick_bench(warmup_iters = 1, iters = 10)]
fn quick_bench_detect_rho_opiuchi(b: bench::Bencher) {
    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping bench");
        return;
    };

    let image_path = cal_dir.join("rho-opiuchi.jpg");
    if !image_path.exists() {
        panic!("rho-opiuchi.jpg not found in {:?}", cal_dir);
    }

    // Preload image outside of benchmark loop
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
    let mut config = StarDetectionConfig::default().precise_ground();
    config.centroid.method = CentroidMethod::GaussianFit;
    let mut detector = StarDetector::from_config(config);

    b.bench(|| detector.detect(&astro_image));
}
