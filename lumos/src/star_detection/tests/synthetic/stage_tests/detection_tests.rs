//! Detection/thresholding stage tests.
//!
//! Tests the peak detection and thresholding logic.

use crate::star_detection::config::Config;
use crate::star_detection::detector::stages::detect_test_utils::detect_stars_test;
use crate::star_detection::tests::common::output::image_writer::{
    gray_to_rgb_image_stretched, save_grayscale, save_image,
};
use crate::star_detection::tests::synthetic::Scenario;
use crate::testing::{estimate_background, init_tracing};
use common::test_utils::test_output_path;
use glam::Vec2;
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

use crate::star_detection::tests::synthetic::stage_tests::TILE_SIZE;

/// Create a detection overlay image showing candidates.
fn create_detection_overlay(
    pixels: &[f32],
    width: usize,
    height: usize,
    candidates: &[(usize, usize)],
    ground_truth: &[(f32, f32)],
) -> imaginarium::Image {
    let mut img = gray_to_rgb_image_stretched(pixels, width, height);

    // Draw ground truth in blue
    let blue = Color::rgb(0.3, 0.3, 1.0);
    for (x, y) in ground_truth {
        draw_circle(&mut img, Vec2::new(*x, *y), 8.0, blue, 1.0);
    }

    // Draw candidates in green
    let green = Color::GREEN;
    for &(x, y) in candidates {
        draw_cross(&mut img, Vec2::new(x as f32, y as f32), 3.0, green, 1.0);
    }

    img
}

/// Test detection on sparse star field.
#[test]

fn test_detection_sparse() {
    init_tracing();

    let (width, height) = (256, 256);
    let frame = Scenario {
        num_stars: 15,
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let ground_truth = frame.truth.sources.clone();

    // Save input
    save_grayscale(
        pixels.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_det_sparse_input.png"),
    );

    // Estimate background
    let background = estimate_background(
        &pixels,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Detect candidates
    let det_config = Config::default();
    let candidates = detect_stars_test(&pixels, &background, &det_config);

    println!("Ground truth: {} stars", ground_truth.len());
    println!("Detected candidates: {}", candidates.len());

    // Create overlay image
    let truth_positions: Vec<(f32, f32)> = ground_truth
        .iter()
        .map(|s| (s.pos.x as f32, s.pos.y as f32))
        .collect();
    let candidate_positions: Vec<(usize, usize)> =
        candidates.iter().map(|c| (c.peak.x, c.peak.y)).collect();
    let overlay = create_detection_overlay(
        pixels.pixels(),
        width,
        height,
        &candidate_positions,
        &truth_positions,
    );
    save_image(
        overlay,
        &test_output_path("synthetic_starfield/stage_det_sparse_overlay.png"),
    );

    // Calculate detection rate
    let match_radius = 5.0;
    let mut matched = 0;
    for (tx, ty) in &truth_positions {
        for c in &candidates {
            let dx = c.peak.x as f32 - tx;
            let dy = c.peak.y as f32 - ty;
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

fn test_detection_thresholds() {
    init_tracing();

    let width = 256;
    let height = 256;

    // Field with a mix of bright and faint stars.
    let frame = Scenario {
        num_stars: 50,
        flux: (3.0, 14.0),
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let ground_truth = frame.truth.sources.clone();

    // Save input
    save_grayscale(
        pixels.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_det_thresholds_input.png"),
    );

    // Estimate background
    let background = estimate_background(
        &pixels,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Test different thresholds
    for sigma in [2.0, 3.0, 5.0, 10.0] {
        let det_config = Config {
            sigma_threshold: sigma,
            ..Default::default()
        };
        let candidates = detect_stars_test(&pixels, &background, &det_config);

        let truth_positions: Vec<(f32, f32)> = ground_truth
            .iter()
            .map(|s| (s.pos.x as f32, s.pos.y as f32))
            .collect();
        let candidate_positions: Vec<(usize, usize)> =
            candidates.iter().map(|c| (c.peak.x, c.peak.y)).collect();
        let overlay = create_detection_overlay(
            pixels.pixels(),
            width,
            height,
            &candidate_positions,
            &truth_positions,
        );
        save_image(
            overlay,
            &test_output_path(&format!(
                "synthetic_starfield/stage_det_thresholds_sigma_{:.0}.png",
                sigma
            )),
        );

        // Count matches
        let match_radius = 5.0;
        let mut matched = 0;
        for (tx, ty) in &truth_positions {
            for c in &candidates {
                let dx = c.peak.x as f32 - tx;
                let dy = c.peak.y as f32 - ty;
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

fn test_detection_area_filter() {
    init_tracing();

    let width = 256;
    let height = 256;

    // Field with cosmic rays (small, sharp features) to exercise the area filter.
    let frame = Scenario {
        num_stars: 30,
        cosmic_rays: 20,
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let ground_truth = frame.truth.sources.clone();

    // Save input
    save_grayscale(
        pixels.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_det_area_filter_input.png"),
    );

    // Estimate background
    let background = estimate_background(
        &pixels,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Test with different area filters
    for (min_area, max_area, label) in [
        (3, 1000, "permissive"),
        (5, 500, "default"),
        (9, 200, "strict"),
    ] {
        let det_config = Config {
            min_area,
            max_area,
            ..Default::default()
        };
        let candidates = detect_stars_test(&pixels, &background, &det_config);

        let truth_positions: Vec<(f32, f32)> = ground_truth
            .iter()
            .map(|s| (s.pos.x as f32, s.pos.y as f32))
            .collect();
        let candidate_positions: Vec<(usize, usize)> =
            candidates.iter().map(|c| (c.peak.x, c.peak.y)).collect();
        let overlay = create_detection_overlay(
            pixels.pixels(),
            width,
            height,
            &candidate_positions,
            &truth_positions,
        );
        save_image(
            overlay,
            &test_output_path(&format!(
                "synthetic_starfield/stage_det_area_filter_{}.png",
                label
            )),
        );

        // Count matches and false positives
        let match_radius = 5.0;
        let mut matched = 0;
        for (tx, ty) in &truth_positions {
            for c in &candidates {
                let dx = c.peak.x as f32 - tx;
                let dy = c.peak.y as f32 - ty;
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
