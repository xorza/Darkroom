//! Real data registration tests.
//!
//! These tests load calibrated light frames from `LUMOS_CALIBRATION_DIR/calibrated_lights/`,
//! run star detection, and register them to verify the pipeline end-to-end.
//! Skipped automatically when the env var is not set.

use std::hint::black_box;
use std::path::PathBuf;

use ::bench::quick_bench;

use crate::AstroImage;
use crate::registration::config::Config as RegistrationConfig;
use crate::star_detection::{StarDetector, config::Config};
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
    let star_config = Config::precise_ground();
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

    // Register image 2 to image 1 WITHOUT SIP first (baseline).
    let reg_config = RegistrationConfig {
        transform_type: crate::TransformType::Auto,
        sip_enabled: false,
        ..RegistrationConfig::default()
    };

    let result = crate::registration::register(&result1.stars, &result2.stars, &reg_config)
        .expect("Registration should succeed");

    let baseline_rms = result.rms_error;

    println!("Registration result:");
    println!("  Matched stars: {}", result.num_inliers);
    println!("  RMS error:     {:.4} pixels", baseline_rms);
    println!("  Elapsed:       {:.1} ms", result.elapsed_ms);

    let t = result.transform.translation_components();
    println!("  Translation:   ({:.2}, {:.2})", t.x, t.y);
    println!(
        "  Rotation:      {:.4} rad ({:.2} deg)",
        result.transform.rotation_angle(),
        result.transform.rotation_angle().to_degrees()
    );
    println!("  Scale:         {:.6}", result.transform.scale_factor());

    // Now fit SIP on the SAME inliers from the SAME RANSAC run for fair comparison.
    // Reconstruct inlier positions from match indices.
    let max_stars = reg_config.max_stars;
    let ref_positions: Vec<glam::DVec2> = result1
        .stars
        .iter()
        .take(max_stars)
        .map(|s| s.pos)
        .collect();
    let target_positions: Vec<glam::DVec2> = result2
        .stars
        .iter()
        .take(max_stars)
        .map(|s| s.pos)
        .collect();

    let inlier_ref: Vec<glam::DVec2> = result
        .matched_stars
        .iter()
        .map(|&(ri, _)| ref_positions[ri])
        .collect();
    let inlier_target: Vec<glam::DVec2> = result
        .matched_stars
        .iter()
        .map(|&(_, ti)| target_positions[ti])
        .collect();

    let sip_config = crate::registration::distortion::SipConfig {
        order: 4,
        reference_point: None,
    };

    let sip = crate::registration::distortion::SipPolynomial::fit_from_transform(
        &inlier_ref,
        &inlier_target,
        &result.transform,
        &sip_config,
    );

    if let Some(ref sip) = sip {
        let corrected_residuals =
            sip.compute_corrected_residuals(&inlier_ref, &inlier_target, &result.transform);
        let sip_rms = (corrected_residuals.iter().map(|r| r * r).sum::<f64>()
            / corrected_residuals.len() as f64)
            .sqrt();

        let improvement = (baseline_rms - sip_rms) / baseline_rms * 100.0;

        println!("\nSIP correction (order 4) on same inliers:");
        println!("  Baseline RMS:      {:.4} pixels", baseline_rms);
        println!("  With SIP RMS:      {:.4} pixels", sip_rms);
        println!("  Improvement:       {:.1}%", improvement);
        println!(
            "  Max SIP correction: {:.4} pixels",
            sip.max_correction(img1.width(), img1.height(), 50.0)
        );

        assert!(
            sip_rms <= baseline_rms + 1e-10,
            "SIP should not worsen RMS: baseline={:.4}, sip={:.4}",
            baseline_rms,
            sip_rms
        );
    } else {
        println!("\nSIP fitting failed (not enough inliers for order 3)");
    }

    assert!(
        result.num_inliers >= 5,
        "Expected at least 5 inlier matches, got {}",
        result.num_inliers
    );
    assert!(
        result.rms_error < 1.5,
        "Expected RMS error < 1.5 pixels, got {:.4}",
        result.rms_error
    );

    // Warp img2 to align with img1 and measure time
    let warp_start = std::time::Instant::now();
    let mut warped = img2.clone();
    crate::registration::warp(&img2, &mut warped, &result.transform, &reg_config);
    let warp_elapsed = warp_start.elapsed();

    println!(
        "\nWarp result: {}x{} image warped in {:.1} ms",
        warped.width(),
        warped.height(),
        warp_elapsed.as_secs_f64() * 1000.0
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

    let config = Config::default();
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
    let star_config = Config::default();
    let mut detector = StarDetector::from_config(star_config);
    let result1 = detector.detect(&img1);
    let result2 = detector.detect(&img2);

    let reg_config = RegistrationConfig::default();

    b.bench(|| {
        black_box(crate::registration::register(
            black_box(&result1.stars),
            black_box(&result2.stars),
            &reg_config,
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_warp(b: ::bench::Bencher) {
    let Some((img1, img2)) = load_two_calibrated_lights() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };

    // Pre-detect stars and register (not part of the benchmark)
    let star_config = Config::default();
    let mut detector = StarDetector::from_config(star_config);
    let result1 = detector.detect(&img1);
    let result2 = detector.detect(&img2);

    let reg_config = RegistrationConfig::default();
    let registration = crate::registration::register(&result1.stars, &result2.stars, &reg_config)
        .expect("Registration should succeed");

    let mut warped = img2.clone();

    b.bench(|| {
        crate::registration::warp(
            black_box(&img2),
            black_box(&mut warped),
            black_box(&registration.transform),
            black_box(&reg_config),
        )
    });
}
