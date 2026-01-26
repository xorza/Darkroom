//! Background estimation stage tests.
//!
//! Tests the background estimation with various synthetic backgrounds.

use crate::star_detection::background::estimate_background;
use crate::star_detection::visual_tests::generators::{
    NebulaConfig, StarFieldConfig, add_gradient_background, add_nebula_background,
    add_uniform_background, add_vignette_background, generate_star_field,
};
use crate::star_detection::visual_tests::output::save_grayscale_png;
use crate::testing::init_tracing;
use common::test_utils::test_output_path;

/// Default tile size for background estimation
const TILE_SIZE: usize = 64;

/// Test background estimation on uniform background.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_background_uniform() {
    init_tracing();

    let width = 512;
    let height = 512;
    let bg_level = 0.15;

    // Create uniform background with some stars
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 30,
        background_level: bg_level,
        noise_sigma: 0.02,
        ..Default::default()
    };
    let (pixels, _ground_truth) = generate_star_field(&config);

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Save input image
    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_uniform_input.png"),
    );

    // Save background map
    let bg_pixels = background.background.clone();
    save_grayscale_png(
        &bg_pixels,
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
    save_grayscale_png(
        &subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_uniform_subtracted.png"),
    );

    // Verify background level is approximately correct
    let mean_bg: f32 = bg_pixels.iter().sum::<f32>() / bg_pixels.len() as f32;
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
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_background_gradient() {
    init_tracing();

    let width = 512;
    let height = 512;

    // Create gradient background
    let mut pixels = vec![0.0f32; width * height];
    add_gradient_background(&mut pixels, width, height, 0.05, 0.25, 0.0);

    // Add some stars
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 30,
        background_level: 0.0, // We already have background
        noise_sigma: 0.02,
        ..Default::default()
    };
    let (star_pixels, _) = generate_star_field(&config);

    // Combine (add stars to gradient)
    for (p, s) in pixels.iter_mut().zip(star_pixels.iter()) {
        *p = (*p + s - 0.1).clamp(0.0, 1.0); // Subtract default bg from stars
    }

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Save images
    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_gradient_input.png"),
    );

    let bg_pixels = background.background.clone();
    save_grayscale_png(
        &bg_pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_gradient_background.png"),
    );

    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    save_grayscale_png(
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
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_background_vignette() {
    init_tracing();

    let width = 512;
    let height = 512;

    // Create vignette background
    let mut pixels = vec![0.0f32; width * height];
    add_vignette_background(&mut pixels, width, height, 0.2, 0.05, 2.0);

    // Add some stars
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 30,
        background_level: 0.0,
        noise_sigma: 0.02,
        ..Default::default()
    };
    let (star_pixels, _) = generate_star_field(&config);

    for (p, s) in pixels.iter_mut().zip(star_pixels.iter()) {
        *p = (*p + s - 0.1).clamp(0.0, 1.0);
    }

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Save images
    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_vignette_input.png"),
    );

    let bg_pixels = background.background.clone();
    save_grayscale_png(
        &bg_pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_vignette_background.png"),
    );

    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    save_grayscale_png(
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
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_background_nebula() {
    init_tracing();

    let width = 512;
    let height = 512;

    // Create nebula background
    let mut pixels = vec![0.0f32; width * height];
    add_uniform_background(&mut pixels, 0.1);
    add_nebula_background(
        &mut pixels,
        width,
        height,
        &NebulaConfig {
            center_x: 0.5, // Center of image (fraction)
            center_y: 0.5,
            radius: 0.3, // 30% of diagonal
            amplitude: 0.3,
            softness: 2.0,
            aspect_ratio: 1.2,
            angle: 0.3,
        },
    );

    // Add some stars
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 40,
        background_level: 0.0,
        noise_sigma: 0.02,
        ..Default::default()
    };
    let (star_pixels, _) = generate_star_field(&config);

    for (p, s) in pixels.iter_mut().zip(star_pixels.iter()) {
        *p = (*p + s - 0.1).clamp(0.0, 1.0);
    }

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Save images
    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_nebula_input.png"),
    );

    let bg_pixels = background.background.clone();
    save_grayscale_png(
        &bg_pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_nebula_background.png"),
    );

    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    save_grayscale_png(
        &subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_bg_nebula_subtracted.png"),
    );

    println!("Visual inspection required for nebula test");
}
