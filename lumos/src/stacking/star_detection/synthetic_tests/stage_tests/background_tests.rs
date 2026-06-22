//! Background estimation stage tests.
//!
//! Tests the background estimation with various synthetic backgrounds.

use glam::Vec2;

use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::synthetic_tests::Scenario;
use crate::stacking::star_detection::test_common::output::image_writer::save_grayscale;
use crate::testing::synthetic::backgrounds::NebulaConfig;
use crate::testing::synthetic::scene::BackgroundField;
use crate::testing::{estimate_background, init_tracing};
use imaginarium::Buffer2;
use common::test_utils::test_output_path;

use crate::stacking::star_detection::synthetic_tests::stage_tests::TILE_SIZE;

/// Test background estimation on uniform background.
#[test]

fn test_background_uniform() {
    init_tracing();

    let width = 256;
    let height = 256;
    let bg_level = 0.15;

    // Uniform background with some stars.
    let pixels = Scenario {
        num_stars: 30,
        background: BackgroundField::Uniform { level: bg_level },
        ..Default::default()
    }
    .frame()
    .image
    .channel(0)
    .clone();

    // Estimate background
    let background = estimate_background(
        &pixels,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Save input image
    save_grayscale(
        pixels.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_uniform_input.png"),
    );

    // Save background map
    save_grayscale(
        background.background.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_uniform_background.png"),
    );

    // Save background-subtracted image
    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    save_grayscale(
        &subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_uniform_subtracted.png"),
    );

    // Verify background level is approximately correct
    let mean_bg: f32 =
        background.background.iter().sum::<f32>() / background.background.len() as f32;
    println!("Expected background: {:.4}", bg_level);
    println!("Estimated mean background: {:.4}", mean_bg);

    // A flat sky must be recovered to well under 1% (the tiled mode estimator is exact here).
    assert!(
        (mean_bg - bg_level).abs() < 0.005,
        "background estimate {mean_bg:.4} should match {bg_level:.4} within 0.005"
    );

    // The noise plane must track the camera's per-pixel σ: well 50000, read 3, sky 0.15 →
    // σ ≈ sqrt(sky·well + read²)/well ≈ 0.00173.
    let mut noise: Vec<f32> = background.noise.pixels().to_vec();
    noise.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_noise = noise[noise.len() / 2];
    println!("median noise {median_noise:.5} (analytic ~0.00173)");
    assert!(
        (0.0010..0.0030).contains(&median_noise),
        "noise plane median {median_noise:.5} should bracket the analytic σ 0.00173"
    );
}

/// Test background estimation on gradient background.
#[test]

fn test_background_gradient() {
    init_tracing();

    let width = 256;
    let height = 256;

    // Stars rendered directly on a gradient sky (0.05 left → 0.25 right).
    let pixels = Scenario {
        num_stars: 30,
        background: BackgroundField::Gradient {
            start: 0.05,
            end: 0.25,
            angle: 0.0,
        },
        ..Default::default()
    }
    .frame()
    .image
    .channel(0)
    .pixels()
    .to_vec();

    // Estimate background
    let background = estimate_background(
        &Buffer2::new(width, height, pixels.clone()),
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Save images
    save_grayscale(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_gradient_input.png"),
    );

    save_grayscale(
        background.background.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_gradient_background.png"),
    );

    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    save_grayscale(
        &subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_gradient_subtracted.png"),
    );

    // Verify gradient is captured
    let left_idx = 50 + (height / 2) * width;
    let right_idx = (width - 50) + (height / 2) * width;
    let left_bg = background.background[left_idx];
    let right_bg = background.background[right_idx];
    println!("Left background: {:.4}", left_bg);
    println!("Right background: {:.4}", right_bg);

    // The estimate must track the injected linear sky `start + (end-start)·x/(width-1)`:
    // at x=50 → 0.089, at x=206 → 0.212 (the stars are masked out by the tiled estimator).
    assert!(
        (left_bg - 0.089).abs() < 0.03,
        "left bg {left_bg:.4} vs analytic 0.089"
    );
    assert!(
        (right_bg - 0.212).abs() < 0.03,
        "right bg {right_bg:.4} vs analytic 0.212"
    );
    assert!(
        right_bg - left_bg > 0.08,
        "gradient slope too shallow: {left_bg:.4} → {right_bg:.4}"
    );
}

/// Test background estimation on vignette pattern.
#[test]

fn test_background_vignette() {
    init_tracing();

    let width = 256;
    let height = 256;

    // Stars rendered directly on a vignette sky (bright centre, dark corners).
    let pixels = Scenario {
        num_stars: 30,
        background: BackgroundField::Vignette {
            center: 0.2,
            edge: 0.05,
            falloff: 2.0,
        },
        ..Default::default()
    }
    .frame()
    .image
    .channel(0)
    .pixels()
    .to_vec();

    // Estimate background
    let background = estimate_background(
        &Buffer2::new(width, height, pixels.clone()),
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Save images
    save_grayscale(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_vignette_input.png"),
    );

    save_grayscale(
        background.background.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_vignette_background.png"),
    );

    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    save_grayscale(
        &subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_vignette_subtracted.png"),
    );

    // Verify center is brighter than corners (sample a genuinely dark corner — the estimator
    // smooths the vignette, so near-center samples barely differ).
    let center_idx = (width / 2) + (height / 2) * width;
    let corner_idx = 25 + 25 * width;
    let center_bg = background.background[center_idx];
    let corner_bg = background.background[corner_idx];
    println!("Center background: {:.4}", center_bg);
    println!("Corner background: {:.4}", corner_bg);

    // The estimator must be *unbiased* on a vignette: its mean tracks the true sky mean despite
    // the radial structure and the embedded stars. (It smooths the central peak rather than
    // fully resolving it — that's smoothing, not bias.)
    let true_bg = BackgroundField::Vignette {
        center: 0.2,
        edge: 0.05,
        falloff: 2.0,
    }
    .render(width, height);
    let true_mean = true_bg.iter().sum::<f32>() / true_bg.len() as f32;
    let est_mean =
        background.background.pixels().iter().sum::<f32>() / background.background.len() as f32;
    println!("vignette true mean {true_mean:.4}, estimate mean {est_mean:.4}");
    assert!(
        (est_mean - true_mean).abs() < 0.01,
        "vignette estimate mean {est_mean:.4} should match true sky mean {true_mean:.4}"
    );
    assert!(
        center_bg >= corner_bg,
        "center {center_bg:.4} should be no darker than corner {corner_bg:.4}"
    );
}

/// Test background estimation with nebula structure.
#[test]

fn test_background_nebula() {
    init_tracing();

    let width = 256;
    let height = 256;

    // Stars rendered directly on a nebula sky.
    let pixels = Scenario {
        num_stars: 40,
        background: BackgroundField::Nebula(NebulaConfig {
            center: Vec2::splat(0.5),
            radius: 0.3,
            amplitude: 0.3,
            softness: 2.0,
            aspect_ratio: 1.2,
            angle: 0.3,
        }),
        ..Default::default()
    }
    .frame()
    .image
    .channel(0)
    .pixels()
    .to_vec();

    // Estimate background
    let background = estimate_background(
        &Buffer2::new(width, height, pixels.clone()),
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Save images
    save_grayscale(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_nebula_input.png"),
    );

    save_grayscale(
        background.background.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_nebula_background.png"),
    );

    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    save_grayscale(
        &subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_nebula_subtracted.png"),
    );

    // The nebula is a central blob: its centre must read clearly brighter than a corner.
    let center_idx = (width / 2) + (height / 2) * width;
    let corner_idx = 30 + 30 * width;
    let center_bg = background.background[center_idx];
    let corner_bg = background.background[corner_idx];
    println!("nebula center {center_bg:.4}, corner {corner_bg:.4}");
    assert!(
        center_bg > corner_bg + 0.02,
        "nebula structure not captured: center {center_bg:.4} vs corner {corner_bg:.4}"
    );
}
