//! Real data registration tests.
//!
//! These tests load the RAW light frames of the bundled dataset, run star detection, and
//! register them to verify the pipeline end to end.

use std::time::Instant;

use crate::math::size2us::Size2us;
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};
use crate::stacking::registration::register;
use crate::stacking::registration::resample::warp;
use crate::stacking::registration::transform::TransformModel;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::real_data::{LightPair, first_and_last_lights};

#[test]
fn register_two_lights() {
    let LightPair {
        first: img1,
        last: img2,
    } = first_and_last_lights();

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
        transform_type: TransformModel::Auto,
        sip: None,
        ..RegistrationConfig::default()
    };

    let result =
        register(&result1.stars, &result2.stars, &reg_config).expect("Registration should succeed");

    let baseline_rms = result.rms_error();

    println!("Registration result:");
    println!("  Matched stars: {}", result.num_inliers());
    println!("  RMS error:     {baseline_rms:.4} pixels");
    println!("  Elapsed:       {:.1} ms", result.elapsed_ms());

    let t = result.transform().translation_components();
    println!("  Translation:   ({:.2}, {:.2})", t.x, t.y);
    println!(
        "  Rotation:      {:.4} rad ({:.2} deg)",
        result.transform().rotation_angle(),
        result.transform().rotation_angle().to_degrees()
    );
    println!("  Scale:         {:.6}", result.transform().scale_factor());

    // Now fit SIP on the SAME inliers from the SAME RANSAC run for fair comparison. A match
    // indexes the star slices `register` was handed.
    let inlier_ref: Vec<glam::DVec2> = result
        .matched_stars()
        .iter()
        .map(|star_match| result1.stars[star_match.indices.reference].pos)
        .collect();
    let inlier_target: Vec<glam::DVec2> = result
        .matched_stars()
        .iter()
        .map(|star_match| result2.stars[star_match.indices.target].pos)
        .collect();

    let sip_config = SipConfig {
        order: 4,
        reference_point: None,
        ..Default::default()
    };

    let sip = SipPolynomial::fit_from_transform(
        &inlier_ref,
        &inlier_target,
        &result.transform(),
        &sip_config,
    )
    .unwrap();

    let corrected_residuals =
        sip.polynomial
            .corrected_residuals(&inlier_ref, &inlier_target, &result.transform());
    let sip_rms = (corrected_residuals.iter().map(|r| r * r).sum::<f64>()
        / corrected_residuals.len() as f64)
        .sqrt();

    let improvement = (baseline_rms - sip_rms) / baseline_rms * 100.0;

    println!("\nSIP correction (order 4) on same inliers:");
    println!("  Baseline RMS:      {baseline_rms:.4} pixels");
    println!("  With SIP RMS:      {sip_rms:.4} pixels");
    println!("  Improvement:       {improvement:.1}%");
    println!(
        "  Max SIP correction: {:.4} pixels",
        sip.polynomial
            .max_grid_correction(Size2us::new(img1.width(), img1.height()), 50.0)
    );

    assert!(
        sip_rms <= baseline_rms + 1e-10,
        "SIP should not worsen RMS: baseline={baseline_rms:.4}, sip={sip_rms:.4}"
    );

    assert!(
        result.num_inliers() >= 5,
        "Expected at least 5 inlier matches, got {}",
        result.num_inliers()
    );
    assert!(
        result.rms_error() < 1.5,
        "Expected RMS error < 1.5 pixels, got {:.4}",
        result.rms_error()
    );

    // Warp img2 to align with img1 and measure time
    let warp_start = Instant::now();
    let warped = warp(&img2, &result.warp_transform(), reg_config.warp).image;
    let warp_elapsed = warp_start.elapsed();

    println!(
        "\nWarp result: {}x{} image warped in {:.1} ms",
        warped.width(),
        warped.height(),
        warp_elapsed.as_secs_f64() * 1000.0
    );
}
