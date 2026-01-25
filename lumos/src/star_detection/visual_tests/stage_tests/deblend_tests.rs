//! Deblending stage tests.
//!
//! Tests the star deblending logic for overlapping/blended stars.

use crate::star_detection::StarDetectionConfig;
use crate::star_detection::background::estimate_background;
use crate::star_detection::detection::detect_stars;
use crate::star_detection::visual_tests::generators::{fwhm_to_sigma, render_gaussian_star};
use crate::star_detection::visual_tests::output::save_grayscale_png;
use crate::testing::init_tracing;
use common::test_utils::test_output_path;
use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_cross_mut, draw_hollow_circle_mut};

/// Default tile size for background estimation
const TILE_SIZE: usize = 64;

/// Test deblending on a pair of overlapping stars.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
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

    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("stage_deblend_pair_input.png"),
    );

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Run detection with deblending enabled
    let config = StarDetectionConfig {
        expected_fwhm: fwhm,
        multi_threshold_deblend: true,
        ..Default::default()
    };

    let candidates = detect_stars(&pixels, width, height, &background, &config);

    // Create overlay
    let mut img = RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let idx = y as usize * width + x as usize;
        let v = (pixels[idx].clamp(0.0, 1.0) * 255.0) as u8;
        Rgb([v, v, v])
    });

    // Draw true positions in blue
    let blue = Rgb([80u8, 80, 255]);
    draw_hollow_circle_mut(&mut img, (star1_x as i32, star_y as i32), 6, blue);
    draw_hollow_circle_mut(&mut img, (star2_x as i32, star_y as i32), 6, blue);

    // Draw detected candidates in green
    let green = Rgb([0u8, 255, 0]);
    for c in &candidates {
        draw_cross_mut(&mut img, green, c.peak_x as i32, c.peak_y as i32);
    }

    img.save(test_output_path("stage_deblend_pair_overlay.png"))
        .unwrap();

    println!(
        "Star pair separation: {:.1}px ({:.1} FWHM)",
        separation,
        separation / fwhm
    );
    println!("Detected candidates: {}", candidates.len());
    for (i, c) in candidates.iter().enumerate() {
        println!("  Candidate {}: ({}, {})", i, c.peak_x, c.peak_y);
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
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_deblend_chain() {
    init_tracing();

    let width = 512;
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

    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("stage_deblend_chain_input.png"),
    );

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Run detection with deblending enabled
    let config = StarDetectionConfig {
        expected_fwhm: fwhm,
        multi_threshold_deblend: true,
        ..Default::default()
    };

    let candidates = detect_stars(&pixels, width, height, &background, &config);

    // Create overlay
    let mut img = RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let idx = y as usize * width + x as usize;
        let v = (pixels[idx].clamp(0.0, 1.0) * 255.0) as u8;
        Rgb([v, v, v])
    });

    // Draw true positions in blue
    let blue = Rgb([80u8, 80, 255]);
    for (x, y) in &true_positions {
        draw_hollow_circle_mut(&mut img, (*x as i32, *y as i32), 6, blue);
    }

    // Draw detected candidates in green
    let green = Rgb([0u8, 255, 0]);
    for c in &candidates {
        draw_cross_mut(&mut img, green, c.peak_x as i32, c.peak_y as i32);
    }

    img.save(test_output_path("stage_deblend_chain_overlay.png"))
        .unwrap();

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
#[cfg_attr(not(feature = "slow-tests"), ignore)]
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

    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("stage_deblend_unequal_input.png"),
    );

    // Estimate background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    // Run detection with deblending enabled
    let config = StarDetectionConfig {
        expected_fwhm: fwhm,
        multi_threshold_deblend: true,
        deblend_min_contrast: 0.005, // Lower contrast to catch faint companion
        ..Default::default()
    };

    let candidates = detect_stars(&pixels, width, height, &background, &config);

    // Create overlay
    let mut img = RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let idx = y as usize * width + x as usize;
        let v = (pixels[idx].clamp(0.0, 1.0) * 255.0) as u8;
        Rgb([v, v, v])
    });

    // Draw true positions in blue
    let blue = Rgb([80u8, 80, 255]);
    draw_hollow_circle_mut(&mut img, (star1_x as i32, star_y as i32), 8, blue); // Bigger for bright
    draw_hollow_circle_mut(&mut img, (star2_x as i32, star_y as i32), 5, blue); // Smaller for faint

    // Draw detected candidates in green
    let green = Rgb([0u8, 255, 0]);
    for c in &candidates {
        draw_cross_mut(&mut img, green, c.peak_x as i32, c.peak_y as i32);
    }

    img.save(test_output_path("stage_deblend_unequal_overlay.png"))
        .unwrap();

    println!(
        "Unequal pair: bright + faint (4:1 ratio), separation: {:.1}px",
        separation
    );
    println!("Detected candidates: {}", candidates.len());
    for (i, c) in candidates.iter().enumerate() {
        println!("  Candidate {}: ({}, {})", i, c.peak_x, c.peak_y);
    }

    // Should detect both stars
    assert!(
        candidates.len() >= 2,
        "Should detect both stars from unequal pair, got {}",
        candidates.len()
    );
}
