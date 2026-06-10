//! Background estimation stage tests.
//!
//! Tests the background estimation with various synthetic backgrounds.

use glam::Vec2;

use crate::star_detection::config::Config;
use crate::star_detection::tests::common::output::image_writer::save_grayscale;
use crate::star_detection::tests::synthetic::Scenario;
use crate::testing::synthetic::backgrounds::NebulaConfig;
use crate::testing::synthetic::scene::BackgroundField;
use crate::testing::{estimate_background, init_tracing};
use common::Buffer2;
use common::test_utils::test_output_path;

use crate::star_detection::tests::synthetic::stage_tests::TILE_SIZE;

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

    // Should be within 20% of true value
    assert!(
        (mean_bg - bg_level).abs() < bg_level * 0.2,
        "Background estimate {:.4} too far from true {:.4}",
        mean_bg,
        bg_level
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

    // Right should be brighter than left
    assert!(
        right_bg > left_bg,
        "Gradient not captured: left={:.4} right={:.4}",
        left_bg,
        right_bg
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

    // Verify center is brighter than corners
    let center_idx = (width / 2) + (height / 2) * width;
    let corner_idx = 50 + 50 * width;
    let center_bg = background.background[center_idx];
    let corner_bg = background.background[corner_idx];
    println!("Center background: {:.4}", center_bg);
    println!("Corner background: {:.4}", corner_bg);

    assert!(
        center_bg > corner_bg,
        "Vignette not captured: center={:.4} corner={:.4}",
        center_bg,
        corner_bg
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

    println!("Visual inspection required for nebula test");
}
