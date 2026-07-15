//! Real data registration tests.
//!
//! These tests load calibrated light frames from `test_data/lumos_data/calibrated_lights/`,
//! run star detection, and register them to verify the pipeline end-to-end.
//! Skipped automatically when the env var is not set.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use ::quickbench::quick_bench;

use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};
use crate::stacking::registration::register;
use crate::stacking::registration::resample::warp;
use crate::stacking::star_detection::config::{CentroidMethod, Config, NoiseModel};
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::calibration_dir;
use crate::{AstroImage, TransformType};

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

/// Load the first and last calibrated light frames from the sample data directory.
/// Returns None if there are fewer than 2 lights.
fn load_two_calibrated_lights() -> Option<(AstroImage, AstroImage)> {
    let cal_dir = calibration_dir();
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
#[cfg_attr(not(feature = "real-data"), ignore)]
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
    let mut detector = StarDetector::from_config(star_config).unwrap();

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
        transform_type: TransformType::Auto,
        sip: None,
        ..RegistrationConfig::default()
    };

    let result =
        register(&result1.stars, &result2.stars, &reg_config).expect("Registration should succeed");

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
    let max_stars = reg_config.matching.max_stars;
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
        .map(|star_match| ref_positions[star_match.reference])
        .collect();
    let inlier_target: Vec<glam::DVec2> = result
        .matched_stars
        .iter()
        .map(|star_match| target_positions[star_match.target])
        .collect();

    let sip_config = SipConfig {
        order: 4,
        reference_point: None,
        ..Default::default()
    };

    let sip = SipPolynomial::fit_from_transform(
        &inlier_ref,
        &inlier_target,
        &result.transform,
        &sip_config,
    )
    .unwrap();

    let corrected_residuals =
        sip.polynomial
            .compute_corrected_residuals(&inlier_ref, &inlier_target, &result.transform);
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
        sip.polynomial
            .max_correction(img1.width(), img1.height(), 50.0)
    );

    assert!(
        sip_rms <= baseline_rms + 1e-10,
        "SIP should not worsen RMS: baseline={:.4}, sip={:.4}",
        baseline_rms,
        sip_rms
    );

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
    let warp_start = Instant::now();
    let warped = warp(&img2, &result.warp_transform(), &reg_config.warp).image;
    let warp_elapsed = warp_start.elapsed();

    println!(
        "\nWarp result: {}x{} image warped in {:.1} ms",
        warped.width(),
        warped.height(),
        warp_elapsed.as_secs_f64() * 1000.0
    );
}

/// Load all calibrated light frames and their file paths.
/// Returns None if there are fewer than 2 lights.
fn load_all_calibrated_lights() -> Option<(Vec<AstroImage>, Vec<PathBuf>)> {
    let cal_dir = calibration_dir();
    let lights_dir = cal_dir.join("calibrated_lights");
    if !lights_dir.exists() {
        eprintln!("calibrated_lights directory not found, skipping");
        return None;
    }

    let files = list_image_files(&lights_dir);
    if files.len() < 2 {
        eprintln!(
            "Need at least 2 calibrated lights, found {}, skipping",
            files.len()
        );
        return None;
    }

    let images: Vec<AstroImage> = files
        .iter()
        .map(|p| AstroImage::from_file(p).expect("Failed to load light frame"))
        .collect();
    Some((images, files))
}

#[quick_bench(warmup_iters = 0, iters = 1)]
fn bench_register_and_warp_all(b: ::quickbench::Bencher) {
    let Some((images, paths)) = load_all_calibrated_lights() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };

    let cal_dir = calibration_dir();
    let output_dir = cal_dir.join("registered_lights");
    std::fs::create_dir_all(&output_dir).expect("Failed to create registered_lights directory");

    println!("Loaded {} calibrated lights", images.len());

    b.bench(|| {
        let star_config = Config::precise_ground();
        let mut detector = StarDetector::from_config(star_config).unwrap();

        // Detect stars in all frames
        let detections: Vec<_> = images.iter().map(|img| detector.detect(img)).collect();

        let reg_config = RegistrationConfig::default();
        let ref_stars = &detections[0].stars;

        println!(
            "Reference: {:?} ({} stars)",
            paths[0].file_name().unwrap(),
            ref_stars.len()
        );

        // Save reference frame as-is
        let ref_output = output_dir.join(paths[0].file_name().unwrap());
        images[0]
            .save(&ref_output)
            .expect("Failed to save reference frame");

        // Register and warp each subsequent frame
        for i in 1..images.len() {
            let name = paths[i].file_name().unwrap();
            let target_stars = &detections[i].stars;

            let result = match register(ref_stars, target_stars, &reg_config) {
                Ok(r) => r,
                Err(e) => {
                    println!("  {:?}: FAILED ({:?}), skipping", name, e);
                    continue;
                }
            };

            println!(
                "  {:?}: {} inliers, RMS {:.4} px, {:.1} ms",
                name, result.num_inliers, result.rms_error, result.elapsed_ms,
            );

            let warped = warp(&images[i], &result.warp_transform(), &reg_config.warp).image;

            let output_path = output_dir.join(name);
            warped
                .save(&output_path)
                .expect("Failed to save warped frame");
        }

        println!(
            "Saved {} registered frames to {:?}",
            images.len(),
            output_dir
        );
    });
}

#[quick_bench(warmup_iters = 3, iters = 30)]
fn bench_register_stars(b: ::quickbench::Bencher) {
    let Some((img1, img2)) = load_two_calibrated_lights() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };

    // Pre-detect stars (not part of the benchmark)
    let star_config = Config::default();
    let mut detector = StarDetector::from_config(star_config).unwrap();
    let result1 = detector.detect(&img1);
    let result2 = detector.detect(&img2);

    let reg_config = RegistrationConfig::default();

    b.bench(|| {
        black_box(register(
            black_box(&result1.stars),
            black_box(&result2.stars),
            &reg_config,
        ))
    });
}

/// PR1 validation: inverse-variance-weighted PSF fitting should not worsen (and ideally
/// improves) registration RMS vs unweighted, by producing lower-variance sub-pixel
/// centroids. Runs the calibrated pair through `GaussianFit` with and without a
/// `NoiseModel`, registers each, and compares.
#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn test_weighted_fit_registration_rms() {
    let Some((img1, img2)) = load_two_calibrated_lights() else {
        return;
    };

    // Normalized-domain gain ≈ phys_gain × white_level for a 14-bit sensor; read noise
    // small. The exact value isn't critical here — it only has to make the weighting active.
    let noise_model = NoiseModel::new(30000.0, 30.0);

    let register_with = |noise: Option<NoiseModel>| -> (f64, usize) {
        let mut config = Config::precise_ground();
        config.measurement.centroid_method = CentroidMethod::GaussianFit;
        config.measurement.noise_model = noise;
        let mut detector = StarDetector::from_config(config).unwrap();
        let s1 = detector.detect(&img1).stars;
        let s2 = detector.detect(&img2).stars;
        let reg_config = RegistrationConfig {
            transform_type: TransformType::Auto,
            sip: None,
            ..RegistrationConfig::default()
        };
        let r = register(&s1, &s2, &reg_config).expect("registration should succeed");
        (r.rms_error, r.num_inliers)
    };

    let (unweighted_rms, unweighted_n) = register_with(None);
    let (weighted_rms, weighted_n) = register_with(Some(noise_model));

    println!("PR1 weighted-fit registration:");
    println!("  unweighted: RMS {unweighted_rms:.4} px, {unweighted_n} matches");
    println!("  weighted:   RMS {weighted_rms:.4} px, {weighted_n} matches");

    // Weighting must not meaningfully worsen registration (5% slack for noise).
    assert!(
        weighted_rms <= unweighted_rms * 1.05,
        "weighted RMS {weighted_rms:.4} should be ≤ unweighted {unweighted_rms:.4} ×1.05"
    );
}
