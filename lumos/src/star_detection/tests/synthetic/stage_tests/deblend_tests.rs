//! Deblending stage tests.
//!
//! Tests the star deblending logic for overlapping/blended stars.

use crate::common::Buffer2;
use crate::star_detection::BackgroundEstimate;
use crate::star_detection::config::Config;
use crate::star_detection::detector::stages::detect::detect_stars_test;
use crate::star_detection::tests::common::output::{
    gray_to_rgb_image_stretched, save_grayscale, save_image,
};
use crate::testing::init_tracing;
use crate::testing::synthetic::{fwhm_to_sigma, render_gaussian_star};
use common::test_utils::test_output_path;
use glam::Vec2;
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

/// Default tile size for background estimation
const TILE_SIZE: usize = 64;

/// Test deblending on a pair of overlapping stars.
#[test]

fn test_deblend_star_pair() {
    init_tracing();

    let width = 256;
    let height = 256;
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);

    // Create two stars separated by ~1.5 FWHM (should be deblended)
    let separation = fwhm * 1.5;
    let star1_x = 128.0 - separation / 2.0;
    let star2_x = 128.0 + separation / 2.0;
    let star_y = 128.0;

    let mut pixels = vec![0.1f32; width * height];

    // Both stars with same brightness
    let amplitude = 0.6 / (2.0 * std::f32::consts::PI * sigma * sigma);
    render_gaussian_star(&mut pixels, width, star1_x, star_y, sigma, amplitude);
    render_gaussian_star(&mut pixels, width, star2_x, star_y, sigma, amplitude);

    // Add noise
    let mut rng = 42u64;
    for p in &mut pixels {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u1 = ((rng >> 33) as f32 / (1u64 << 31) as f32).max(1e-10);
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u2 = (rng >> 33) as f32 / (1u64 << 31) as f32;
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        *p += z * 0.01;
        *p = p.clamp(0.0, 1.0);
    }

    save_grayscale(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_deblend_pair_input.png"),
    );

    // Estimate background
    let pixels_buf = Buffer2::new(width, height, pixels.clone());
    let background = crate::testing::estimate_background(
        &pixels_buf,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Run detection with deblending enabled
    let config = Config {
        expected_fwhm: fwhm,
        deblend_n_thresholds: 32,
        ..Default::default()
    };

    let candidates = detect_stars_test(&pixels_buf, None, &background, &config);

    // Create overlay
    let mut img = gray_to_rgb_image_stretched(&pixels, width, height);

    // Draw true positions in blue
    let blue = Color::rgb(0.3, 0.3, 1.0);
    draw_circle(&mut img, Vec2::new(star1_x, star_y), 6.0, blue, 1.0);
    draw_circle(&mut img, Vec2::new(star2_x, star_y), 6.0, blue, 1.0);

    // Draw detected candidates in green
    let green = Color::GREEN;
    for c in &candidates {
        draw_cross(
            &mut img,
            Vec2::new(c.peak.x as f32, c.peak.y as f32),
            3.0,
            green,
            1.0,
        );
    }

    save_image(
        img,
        &test_output_path("synthetic_starfield/stage_deblend_pair_overlay.png"),
    );

    println!(
        "Star pair separation: {:.1}px ({:.1} FWHM)",
        separation,
        separation / fwhm
    );
    println!("Detected candidates: {}", candidates.len());
    for (i, c) in candidates.iter().enumerate() {
        println!("  Candidate {}: ({}, {})", i, c.peak.x, c.peak.y);
    }

    // Should detect 2 separate stars
    assert!(
        candidates.len() >= 2,
        "Should detect at least 2 stars from blended pair, got {}",
        candidates.len()
    );
}

/// Test deblending on a chain of touching stars.
#[test]

fn test_deblend_chain() {
    init_tracing();

    let width = 256;
    let height = 128;
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);

    // Create 5 stars in a chain, each separated by 1 FWHM
    let num_stars = 5;
    let separation = fwhm;
    let start_x = 100.0;
    let star_y = 64.0;

    let mut pixels = vec![0.1f32; width * height];
    let mut true_positions = Vec::new();

    let amplitude = 0.5 / (2.0 * std::f32::consts::PI * sigma * sigma);
    for i in 0..num_stars {
        let x = start_x + i as f32 * separation;
        true_positions.push((x, star_y));
        render_gaussian_star(&mut pixels, width, x, star_y, sigma, amplitude);
    }

    // Add noise
    let mut rng = 42u64;
    for p in &mut pixels {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u1 = ((rng >> 33) as f32 / (1u64 << 31) as f32).max(1e-10);
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u2 = (rng >> 33) as f32 / (1u64 << 31) as f32;
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        *p += z * 0.01;
        *p = p.clamp(0.0, 1.0);
    }

    save_grayscale(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_deblend_chain_input.png"),
    );

    // Estimate background
    let pixels_buf = Buffer2::new(width, height, pixels.clone());
    let background = crate::testing::estimate_background(
        &pixels_buf,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Run detection with deblending enabled
    let config = Config {
        expected_fwhm: fwhm,
        deblend_n_thresholds: 32,
        ..Default::default()
    };

    let candidates = detect_stars_test(&pixels_buf, None, &background, &config);

    // Create overlay
    let mut img = gray_to_rgb_image_stretched(&pixels, width, height);

    // Draw true positions in blue
    let blue = Color::rgb(0.3, 0.3, 1.0);
    for (x, y) in &true_positions {
        draw_circle(&mut img, Vec2::new(*x, *y), 6.0, blue, 1.0);
    }

    // Draw detected candidates in green
    let green = Color::GREEN;
    for c in &candidates {
        draw_cross(
            &mut img,
            Vec2::new(c.peak.x as f32, c.peak.y as f32),
            3.0,
            green,
            1.0,
        );
    }

    save_image(
        img,
        &test_output_path("synthetic_starfield/stage_deblend_chain_overlay.png"),
    );

    println!(
        "Chain of {} stars, separation: {:.1}px ({:.1} FWHM)",
        num_stars,
        separation,
        separation / fwhm
    );
    println!("Detected candidates: {}", candidates.len());

    // Should detect at least half the stars in tight chain
    assert!(
        candidates.len() >= num_stars / 2,
        "Should detect at least {} stars from chain of {}, got {}",
        num_stars / 2,
        num_stars,
        candidates.len()
    );
}

/// Test deblending on unequal brightness pair.
#[test]

fn test_deblend_unequal_pair() {
    init_tracing();

    let width = 256;
    let height = 256;
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);

    // Create two stars: one bright, one faint, separated by ~2 FWHM
    let separation = fwhm * 2.0;
    let star1_x = 128.0 - separation / 2.0;
    let star2_x = 128.0 + separation / 2.0;
    let star_y = 128.0;

    let mut pixels = vec![0.1f32; width * height];

    // Bright star
    let amplitude1 = 0.8 / (2.0 * std::f32::consts::PI * sigma * sigma);
    render_gaussian_star(&mut pixels, width, star1_x, star_y, sigma, amplitude1);

    // Faint star (1/4 brightness)
    let amplitude2 = 0.2 / (2.0 * std::f32::consts::PI * sigma * sigma);
    render_gaussian_star(&mut pixels, width, star2_x, star_y, sigma, amplitude2);

    // Add noise
    let mut rng = 42u64;
    for p in &mut pixels {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u1 = ((rng >> 33) as f32 / (1u64 << 31) as f32).max(1e-10);
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u2 = (rng >> 33) as f32 / (1u64 << 31) as f32;
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        *p += z * 0.01;
        *p = p.clamp(0.0, 1.0);
    }

    save_grayscale(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_deblend_unequal_input.png"),
    );

    // Estimate background
    let pixels_buf = Buffer2::new(width, height, pixels.clone());
    let background = crate::testing::estimate_background(
        &pixels_buf,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    // Run detection with deblending enabled
    let config = Config {
        expected_fwhm: fwhm,
        deblend_n_thresholds: 32,
        deblend_min_contrast: 0.005, // Lower contrast to catch faint companion
        ..Default::default()
    };

    let candidates = detect_stars_test(&pixels_buf, None, &background, &config);

    // Create overlay
    let mut img = gray_to_rgb_image_stretched(&pixels, width, height);

    // Draw true positions in blue
    let blue = Color::rgb(0.3, 0.3, 1.0);
    draw_circle(&mut img, Vec2::new(star1_x, star_y), 8.0, blue, 1.0); // Bigger for bright
    draw_circle(&mut img, Vec2::new(star2_x, star_y), 5.0, blue, 1.0); // Smaller for faint

    // Draw detected candidates in green
    let green = Color::GREEN;
    for c in &candidates {
        draw_cross(
            &mut img,
            Vec2::new(c.peak.x as f32, c.peak.y as f32),
            3.0,
            green,
            1.0,
        );
    }

    save_image(
        img,
        &test_output_path("synthetic_starfield/stage_deblend_unequal_overlay.png"),
    );

    println!(
        "Unequal pair: bright + faint (4:1 ratio), separation: {:.1}px",
        separation
    );
    println!("Detected candidates: {}", candidates.len());
    for (i, c) in candidates.iter().enumerate() {
        println!("  Candidate {}: ({}, {})", i, c.peak.x, c.peak.y);
    }

    // Should detect both stars
    assert!(
        candidates.len() >= 2,
        "Should detect both stars from unequal pair, got {}",
        candidates.len()
    );
}
