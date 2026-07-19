//! Detection/thresholding stage tests.
//!
//! Tests the peak detection and thresholding logic.

use crate::stacking::star_detection::config::{BackgroundConfig, DetectionConfig};
use crate::stacking::star_detection::detector::stages::detect_test_utils::detect_stars_test;
use crate::stacking::star_detection::synthetic_tests::Scenario;
use crate::stacking::star_detection::test_common::output::image_writer::{
    gray_to_rgb_image_stretched, save_grayscale, save_image,
};
use crate::testing::{estimate_background, init_tracing};
use common::test_utils::test_output_path;
use glam::Vec2;
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

use crate::stacking::star_detection::synthetic_tests::stage_tests::{TILE_SIZE, matched_truths};

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
        &BackgroundConfig {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Detect candidates
    let det_config = DetectionConfig::default();
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

    // Field with a mix of bright and faint stars over a shallow (noisy) well, so the faint half
    // sits near the threshold and drops out as σ rises.
    let frame = Scenario {
        num_stars: 50,
        flux: (1.0, 10.0),
        full_well_e: 2_000.0,
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
        &BackgroundConfig {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // The detection threshold is the central knob: raising σ must yield strictly fewer
    // candidates overall and never more matches — a detector ignoring `sigma_threshold` would
    // return a flat count across the sweep.
    let truth_positions: Vec<(f32, f32)> = ground_truth
        .iter()
        .map(|s| (s.pos.x as f32, s.pos.y as f32))
        .collect();
    let mut counts: Vec<(f32, usize, usize)> = Vec::new();
    for sigma in [2.0, 3.0, 5.0, 10.0] {
        let det_config = DetectionConfig {
            sigma_threshold: sigma,
            ..Default::default()
        };
        let candidates = detect_stars_test(&pixels, &background, &det_config);
        let matched = matched_truths(&candidates, &truth_positions, 5.0);
        println!(
            "sigma {sigma}: candidates {}, matched {matched}",
            candidates.len()
        );
        counts.push((sigma, candidates.len(), matched));
    }
    for w in counts.windows(2) {
        assert!(
            w[1].1 <= w[0].1,
            "candidate count must not rise with threshold: σ {}→{} gave {}→{}",
            w[0].0,
            w[1].0,
            w[0].1,
            w[1].1
        );
        assert!(
            w[1].2 <= w[0].2,
            "match count must not rise with threshold: σ {}→{} matched {}→{}",
            w[0].0,
            w[1].0,
            w[0].2,
            w[1].2
        );
    }
    assert!(
        counts[0].1 > counts[3].1,
        "2σ must detect strictly more candidates than 10σ: {} vs {}",
        counts[0].1,
        counts[3].1
    );
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
        &BackgroundConfig {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // The area filter must discriminate small, sharp cosmic rays from real stars: a strict
    // `[9,200]` minimum-area cut removes the single-pixel CR detections that a permissive
    // `[3,1000]` cut keeps, while real stars (larger footprints) survive both.
    let truth_positions: Vec<(f32, f32)> = ground_truth
        .iter()
        .map(|s| (s.pos.x as f32, s.pos.y as f32))
        .collect();
    let run = |min_area: usize, max_area: usize| -> (usize, usize) {
        let det_config = DetectionConfig {
            min_area,
            max_area,
            ..Default::default()
        };
        let candidates = detect_stars_test(&pixels, &background, &det_config);
        let matched = matched_truths(&candidates, &truth_positions, 5.0);
        let false_positives = candidates.len().saturating_sub(matched);
        (matched, false_positives)
    };
    let (permissive_matched, permissive_fp) = run(3, 1000);
    let (strict_matched, strict_fp) = run(9, 200);
    println!(
        "permissive: matched {permissive_matched}, FP {permissive_fp}; strict: matched {strict_matched}, FP {strict_fp}"
    );

    assert!(
        strict_fp < permissive_fp,
        "strict area filter should reject CR false positives: strict FP {strict_fp} vs permissive {permissive_fp}"
    );
    // Real stars survive the strict filter (they are larger than the CR area cut).
    assert!(
        strict_matched >= permissive_matched.saturating_sub(2),
        "strict filter should keep real stars: strict {strict_matched} vs permissive {permissive_matched}"
    );
}
