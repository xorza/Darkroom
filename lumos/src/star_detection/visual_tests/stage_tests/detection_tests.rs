//! Detection/thresholding stage tests.
//!
//! Tests the peak detection and thresholding logic.

use crate::star_detection::StarDetectionConfig;
use crate::star_detection::background::estimate_background;
use crate::star_detection::detection::detect_stars;
use crate::star_detection::visual_tests::generators::{
    StarFieldConfig, generate_star_field, sparse_field_config,
};
use crate::star_detection::visual_tests::output::save_grayscale_png;
use crate::testing::init_tracing;
use common::test_utils::test_output_path;
use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_cross_mut, draw_hollow_circle_mut};

/// Default tile size for background estimation
const TILE_SIZE: usize = 64;

/// Create a detection overlay image showing candidates.
fn create_detection_overlay(
    pixels: &[f32],
    width: usize,
    height: usize,
    candidates: &[(usize, usize)],
    ground_truth: &[(f32, f32)],
) -> RgbImage {
    // Normalize pixels for display
    let min_val = pixels.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = pixels.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (max_val - min_val).max(1e-10);

    let mut img = RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let idx = y as usize * width + x as usize;
        let v = ((pixels[idx] - min_val) / range * 255.0) as u8;
        Rgb([v, v, v])
    });

    // Draw ground truth in blue
    let blue = Rgb([80u8, 80, 255]);
    for (x, y) in ground_truth {
        let cx = x.round() as i32;
        let cy = y.round() as i32;
        draw_hollow_circle_mut(&mut img, (cx, cy), 8, blue);
    }

    // Draw candidates in green
    let green = Rgb([0u8, 255, 0]);
    for &(x, y) in candidates {
        draw_cross_mut(&mut img, green, x as i32, y as i32);
    }

    img
}

/// Test detection on sparse star field.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_detection_sparse() {
    init_tracing();

    let config = sparse_field_config();
    let (pixels, ground_truth) = generate_star_field(&config);

    // Save input
    save_grayscale_png(
        &pixels,
        config.width,
        config.height,
        &test_output_path("stage_det_sparse_input.png"),
    );

    // Estimate background
    let background = estimate_background(&pixels, config.width, config.height, TILE_SIZE);

    // Detect candidates
    let det_config = StarDetectionConfig::default();
    let candidates = detect_stars(
        &pixels,
        config.width,
        config.height,
        &background,
        &det_config,
    );

    println!("Ground truth: {} stars", ground_truth.len());
    println!("Detected candidates: {}", candidates.len());

    // Create overlay image
    let truth_positions: Vec<(f32, f32)> = ground_truth.iter().map(|s| (s.x, s.y)).collect();
    let candidate_positions: Vec<(usize, usize)> =
        candidates.iter().map(|c| (c.peak_x, c.peak_y)).collect();
    let overlay = create_detection_overlay(
        &pixels,
        config.width,
        config.height,
        &candidate_positions,
        &truth_positions,
    );
    overlay
        .save(test_output_path("stage_det_sparse_overlay.png"))
        .unwrap();

    // Calculate detection rate
    let match_radius = 5.0;
    let mut matched = 0;
    for (tx, ty) in &truth_positions {
        for c in &candidates {
            let dx = c.peak_x as f32 - tx;
            let dy = c.peak_y as f32 - ty;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < match_radius {
                matched += 1;
                break;
            }
        }
    }

    let detection_rate = matched as f32 / ground_truth.len() as f32;
    println!("Detection rate: {:.1}%", detection_rate * 100.0);

    assert!(
        detection_rate > 0.9,
        "Detection rate {:.1}% should be > 90%",
        detection_rate * 100.0
    );
}

/// Test detection with different sigma thresholds.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_detection_thresholds() {
    init_tracing();

    let width = 512;
    let height = 512;

    // Create field with mix of bright and faint stars
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 50,
        magnitude_range: (8.0, 14.0), // Wide range
        fwhm_range: (3.0, 4.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        ..Default::default()
    };
    let (pixels, ground_truth) = generate_star_field(&config);

    // Save input
    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("stage_det_thresholds_input.png"),
    );

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Test different thresholds
    for sigma in [2.0, 3.0, 5.0, 10.0] {
        let det_config = StarDetectionConfig {
            detection_sigma: sigma,
            ..Default::default()
        };
        let candidates = detect_stars(&pixels, width, height, &background, &det_config);

        let truth_positions: Vec<(f32, f32)> = ground_truth.iter().map(|s| (s.x, s.y)).collect();
        let candidate_positions: Vec<(usize, usize)> =
            candidates.iter().map(|c| (c.peak_x, c.peak_y)).collect();
        let overlay = create_detection_overlay(
            &pixels,
            width,
            height,
            &candidate_positions,
            &truth_positions,
        );
        overlay
            .save(test_output_path(&format!(
                "stage_det_thresholds_sigma_{:.0}.png",
                sigma
            )))
            .unwrap();

        // Count matches
        let match_radius = 5.0;
        let mut matched = 0;
        for (tx, ty) in &truth_positions {
            for c in &candidates {
                let dx = c.peak_x as f32 - tx;
                let dy = c.peak_y as f32 - ty;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < match_radius {
                    matched += 1;
                    break;
                }
            }
        }

        let detection_rate = matched as f32 / ground_truth.len() as f32;
        let false_positives = candidates.len().saturating_sub(matched);

        println!(
            "Sigma={:.1}: candidates={}, matched={}, FP={}, rate={:.1}%",
            sigma,
            candidates.len(),
            matched,
            false_positives,
            detection_rate * 100.0
        );
    }
}

/// Test detection area filtering.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_detection_area_filter() {
    init_tracing();

    let width = 512;
    let height = 512;

    // Create field with varying star sizes
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 30,
        fwhm_range: (2.0, 6.0), // Wide range of sizes
        magnitude_range: (8.0, 12.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        // Add some cosmic rays (small, sharp features)
        cosmic_ray_count: 20,
        ..Default::default()
    };
    let (pixels, ground_truth) = generate_star_field(&config);

    // Save input
    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("stage_det_area_filter_input.png"),
    );

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Test with different area filters
    for (min_area, max_area, label) in [
        (3, 1000, "permissive"),
        (5, 500, "default"),
        (9, 200, "strict"),
    ] {
        let det_config = StarDetectionConfig {
            min_area,
            max_area,
            ..Default::default()
        };
        let candidates = detect_stars(&pixels, width, height, &background, &det_config);

        let truth_positions: Vec<(f32, f32)> = ground_truth.iter().map(|s| (s.x, s.y)).collect();
        let candidate_positions: Vec<(usize, usize)> =
            candidates.iter().map(|c| (c.peak_x, c.peak_y)).collect();
        let overlay = create_detection_overlay(
            &pixels,
            width,
            height,
            &candidate_positions,
            &truth_positions,
        );
        overlay
            .save(test_output_path(&format!(
                "stage_det_area_filter_{}.png",
                label
            )))
            .unwrap();

        // Count matches and false positives
        let match_radius = 5.0;
        let mut matched = 0;
        for (tx, ty) in &truth_positions {
            for c in &candidates {
                let dx = c.peak_x as f32 - tx;
                let dy = c.peak_y as f32 - ty;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < match_radius {
                    matched += 1;
                    break;
                }
            }
        }

        let detection_rate = matched as f32 / ground_truth.len() as f32;
        let false_positives = candidates.len().saturating_sub(matched);

        println!(
            "{}: area=[{},{}], candidates={}, matched={}, FP={}, rate={:.1}%",
            label,
            min_area,
            max_area,
            candidates.len(),
            matched,
            false_positives,
            detection_rate * 100.0
        );
    }
}
