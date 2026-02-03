//! Real data registration tests.
//!
//! These tests load calibrated light frames from `LUMOS_CALIBRATION_DIR/calibrated_lights/`,
//! run star detection, and register them to verify the pipeline end-to-end.
//! Skipped automatically when the env var is not set.

use std::hint::black_box;
use std::path::PathBuf;

use ::bench::quick_bench;

use crate::AstroImage;
use crate::registration::config::RegistrationConfig;
use crate::registration::pipeline::Registrator;
use crate::star_detection::{StarDetectionConfig, StarDetector};
use crate::testing::calibration_dir;

const IMAGE_EXTENSIONS: &[&str] = &["tiff", "tif", "fit", "fits", "png"];

/// List image files in a directory, sorted by name.
fn list_image_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("Failed to read directory")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            if !path.is_file() {
                return false;
            }
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str())
        })
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}

/// Load one calibrated light frame (the first one).
/// Returns None if `LUMOS_CALIBRATION_DIR` is not set or has no lights.
fn load_first_calibrated_light() -> Option<AstroImage> {
    let cal_dir = calibration_dir()?;
    let lights_dir = cal_dir.join("calibrated_lights");
    if !lights_dir.exists() {
        eprintln!("calibrated_lights directory not found, skipping");
        return None;
    }

    let files = list_image_files(&lights_dir);
    if files.is_empty() {
        eprintln!("No calibrated lights found, skipping");
        return None;
    }

    let img = AstroImage::from_file(&files[0]).expect("Failed to load first light frame");
    Some(img)
}

/// Load the first and last calibrated light frames from the sample data directory.
/// Returns None if `LUMOS_CALIBRATION_DIR` is not set or has fewer than 2 lights.
fn load_two_calibrated_lights() -> Option<(AstroImage, AstroImage)> {
    let cal_dir = calibration_dir()?;
    let lights_dir = cal_dir.join("calibrated_lights");
    if !lights_dir.exists() {
        eprintln!("calibrated_lights directory not found, skipping test");
        return None;
    }

    let files = list_image_files(&lights_dir);
    if files.len() < 2 {
        eprintln!(
            "Need at least 2 calibrated lights, found {}, skipping test",
            files.len()
        );
        return None;
    }

    let first = &files[0];
    let last = &files[files.len() - 1];

    println!("Loading first: {:?}", first.file_name().unwrap());
    let img1 = AstroImage::from_file(first).expect("Failed to load first light frame");
    println!("Loading last:  {:?}", last.file_name().unwrap());
    let img2 = AstroImage::from_file(last).expect("Failed to load last light frame");
    Some((img1, img2))
}

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR with calibrated_lights/ subdirectory
fn test_register_two_calibrated_lights() {
    let Some((img1, img2)) = load_two_calibrated_lights() else {
        return;
    };

    println!(
        "Image 1: {}x{} ({} ch)",
        img1.width(),
        img1.height(),
        img1.channels()
    );
    println!(
        "Image 2: {}x{} ({} ch)",
        img2.width(),
        img2.height(),
        img2.channels()
    );

    // Detect stars with precise Moffat centroids
    let star_config = StarDetectionConfig::default().precise_ground();
    let mut detector = StarDetector::from_config(star_config);

    let result1 = detector.detect(&img1);
    let result2 = detector.detect(&img2);

    println!("Stars in image 1: {}", result1.stars.len());
    println!("Stars in image 2: {}", result2.stars.len());

    assert!(
        result1.stars.len() >= 10,
        "Expected at least 10 stars in image 1, found {}",
        result1.stars.len()
    );
    assert!(
        result2.stars.len() >= 10,
        "Expected at least 10 stars in image 2, found {}",
        result2.stars.len()
    );

    // Register image 2 to image 1 (star detection -> transform, no warping)
    let mut reg_config = RegistrationConfig::default();
    reg_config.transform_type = crate::TransformType::Homography;
    let registrator = Registrator::new(reg_config);

    let result = registrator
        .register_stars(&result1.stars, &result2.stars)
        .expect("Registration should succeed");

    println!("Registration result:");
    println!("  Matched stars: {}", result.num_inliers);
    println!("  RMS error:     {:.4} pixels", result.rms_error);
    println!("  Elapsed:       {:.1} ms", result.elapsed_ms);

    let t = result.transform.translation_components();
    println!("  Translation:   ({:.2}, {:.2})", t.x, t.y);
    println!(
        "  Rotation:      {:.4} rad ({:.2} deg)",
        result.transform.rotation_angle(),
        result.transform.rotation_angle().to_degrees()
    );
    println!("  Scale:         {:.6}", result.transform.scale_factor());

    assert!(
        result.num_inliers >= 6,
        "Expected at least 6 inlier matches, got {}",
        result.num_inliers
    );
    assert!(
        result.rms_error < 1.5,
        "Expected RMS error < 1.5 pixels, got {:.4}",
        result.rms_error
    );
}

// ============================================================================
// Benchmarks
// ============================================================================

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_star_detection_calibrated_light(b: ::bench::Bencher) {
    let Some(image) = load_first_calibrated_light() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };

    let config = StarDetectionConfig::default();
    let mut detector = StarDetector::from_config(config);

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_iters = 3, iters = 30)]
fn bench_register_stars(b: ::bench::Bencher) {
    let Some((img1, img2)) = load_two_calibrated_lights() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };

    // Pre-detect stars (not part of the benchmark)
    let star_config = StarDetectionConfig::default();
    let mut detector = StarDetector::from_config(star_config);
    let result1 = detector.detect(&img1);
    let result2 = detector.detect(&img2);

    let reg_config = RegistrationConfig::default();

    b.bench(|| {
        let registrator = Registrator::new(reg_config.clone());
        black_box(registrator.register_stars(black_box(&result1.stars), black_box(&result2.stars)))
    });
}
