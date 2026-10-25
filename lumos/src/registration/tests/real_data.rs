//! Real data registration tests.
//!
//! These tests load the RAW light frames of the bundled dataset, run star detection, and
//! register them to verify the pipeline end to end.

use std::time::Instant;

use glam::DVec2;

use crate::internals::real_data::{LightPair, first_and_last_lights};
use crate::registration::distortion::sip::SipModel;
use crate::registration::distortion::sip::sip_order::SipOrder;
use crate::registration::point_normalization::centroid;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::resample::warp_config::WarpConfig;
use crate::registration::transform_model::TransformModel;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detector::StarDetector;

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
    let star_config = StarDetectionConfig::precise_ground();
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
        model: TransformModel::Auto { sip: None },
        ..RegistrationConfig::default()
    };

    let result = reg_config
        .registrar()
        .unwrap()
        .register(&result1.stars, &result2.stars)
        .expect("Registration should succeed");

    let baseline_rms = result.rms_error();

    println!("Registration result:");
    println!("  Matched stars: {}", result.inlier_count());
    println!("  RMS error:     {baseline_rms:.4} pixels");

    let t = result.transform().translation_components();
    println!("  Translation:   ({:.2}, {:.2})", t.x, t.y);
    println!(
        "  Rotation:      {:.4} rad ({:.2} deg)",
        result.transform().local_geometry(DVec2::ZERO).rotation,
        result
            .transform()
            .local_geometry(DVec2::ZERO)
            .rotation
            .to_degrees()
    );
    println!(
        "  Stretch:       {:?}",
        result.transform().local_geometry(DVec2::ZERO).stretch
    );

    // Now fit SIP on the SAME inliers from the SAME RANSAC run for fair comparison. A match
    // indexes the star slices `register` was handed.
    let inlier_ref: Vec<DVec2> = result
        .matched_stars()
        .iter()
        .map(|star_match| result1.stars[star_match.idxs.reference].pos)
        .collect();
    let inlier_target: Vec<DVec2> = result
        .matched_stars()
        .iter()
        .map(|star_match| result2.stars[star_match.idxs.target].pos)
        .collect();

    let origin = centroid(&inlier_ref);
    let sip = SipModel {
        order: SipOrder::Four,
        origin,
    }
    .fitted_under(&result.transform(), &inlier_ref, &inlier_target);

    let corrected_residuals =
        sip.corrected_residuals(&inlier_ref, &inlier_target, &result.transform());
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
        sip.max_grid_correction(img1.size(), 50.0)
    );

    assert!(
        sip_rms <= baseline_rms + 1e-10,
        "SIP should not worsen RMS: baseline={baseline_rms:.4}, sip={sip_rms:.4}"
    );

    assert!(
        result.inlier_count() >= 5,
        "Expected at least 5 inlier matches, got {}",
        result.inlier_count()
    );
    assert!(
        result.rms_error() < 1.5,
        "Expected RMS error < 1.5 pixels, got {:.4}",
        result.rms_error()
    );

    // Warp img2 to align with img1 and measure time
    let warp_start = Instant::now();
    let warped = WarpConfig::default()
        .warp(&img2, result.warp_transform())
        .image;
    let warp_elapsed = warp_start.elapsed();

    println!(
        "\nWarp result: {}x{} image warped in {:.1} ms",
        warped.width(),
        warped.height(),
        warp_elapsed.as_secs_f64() * 1000.0
    );
}
