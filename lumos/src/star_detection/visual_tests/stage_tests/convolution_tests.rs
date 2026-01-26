//! Convolution/filtering stage tests.
//!
//! Tests the Gaussian filtering for star enhancement.

use crate::star_detection::background::estimate_background;
use crate::star_detection::constants::fwhm_to_sigma;
use crate::star_detection::convolution::gaussian_convolve;
use crate::star_detection::visual_tests::output::save_grayscale_png;
use crate::testing::init_tracing;
use crate::testing::synthetic::{StarFieldConfig, generate_star_field};
use common::test_utils::test_output_path;

/// Default tile size for background estimation
const TILE_SIZE: usize = 64;

/// Normalize filtered output for visualization (handle negative values).
fn normalize_for_display(pixels: &[f32]) -> Vec<f32> {
    let min_val = pixels.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = pixels.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (max_val - min_val).max(1e-10);

    pixels.iter().map(|&p| (p - min_val) / range).collect()
}

/// Test Gaussian filter response on sparse star field.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_gaussian_filter_sparse() {
    init_tracing();

    let width = 512;
    let height = 512;
    let fwhm = 3.5;

    // Create sparse star field
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 25,
        fwhm_range: (fwhm, fwhm),
        magnitude_range: (8.0, 12.0),
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
        &test_output_path("synthetic_starfield/stage_conv_sparse_input.png"),
    );

    // Estimate and subtract background
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    let bg_subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();

    save_grayscale_png(
        &bg_subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_sparse_bg_subtracted.png"),
    );

    // Apply Gaussian filter (matched filter for star detection)
    let sigma = fwhm_to_sigma(fwhm);
    let filtered = gaussian_convolve(&bg_subtracted, width, height, sigma);

    // Normalize for display
    let filtered_display = normalize_for_display(&filtered);

    save_grayscale_png(
        &filtered_display,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_sparse_filtered.png"),
    );

    // Verify stars are enhanced
    println!("Ground truth stars: {}", ground_truth.len());
    println!("Sigma: {:.2}", sigma);

    // Check that star positions have high filtered values
    let mut star_responses: Vec<f32> = Vec::new();
    for star in &ground_truth {
        let x = star.x.round() as usize;
        let y = star.y.round() as usize;
        if x > 10 && x < width - 10 && y > 10 && y < height - 10 {
            let response = filtered[y * width + x];
            star_responses.push(response);
        }
    }

    let mean_star_response: f32 =
        star_responses.iter().sum::<f32>() / star_responses.len().max(1) as f32;
    let mean_bg_response: f32 = filtered.iter().sum::<f32>() / filtered.len() as f32;

    println!("Mean star response: {:.6}", mean_star_response);
    println!("Mean background response: {:.6}", mean_bg_response);

    // Stars should have significantly higher response
    assert!(
        mean_star_response > mean_bg_response * 3.0,
        "Star response {:.6} should be much higher than background {:.6}",
        mean_star_response,
        mean_bg_response
    );
}

/// Test Gaussian filter with different FWHM values.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_gaussian_filter_fwhm_range() {
    init_tracing();

    let width = 512;
    let height = 512;

    // Create stars with varying FWHM
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 30,
        fwhm_range: (2.0, 6.0), // Wide FWHM range
        magnitude_range: (8.0, 11.0),
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
        &test_output_path("synthetic_starfield/stage_conv_fwhm_range_input.png"),
    );

    // Background subtraction
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    let bg_subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();

    // Test with different kernel sizes
    for target_fwhm in [2.5, 4.0, 5.5] {
        let sigma = fwhm_to_sigma(target_fwhm);
        let filtered = gaussian_convolve(&bg_subtracted, width, height, sigma);
        let filtered_display = normalize_for_display(&filtered);

        save_grayscale_png(
            &filtered_display,
            width,
            height,
            &test_output_path(&format!(
                "synthetic_starfield/stage_conv_fwhm_range_filtered_{:.1}.png",
                target_fwhm
            )),
        );

        println!("FWHM={:.1}: sigma={:.2}", target_fwhm, sigma);
    }

    println!("Ground truth FWHM range: {:.1} - {:.1}", 2.0, 6.0);
    println!("Generated {} stars", ground_truth.len());
}

/// Test Gaussian filter noise rejection.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_gaussian_filter_noise() {
    init_tracing();

    let width = 512;
    let height = 512;
    let fwhm = 3.5;

    // Create star field with high noise
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 20,
        fwhm_range: (fwhm, fwhm),
        magnitude_range: (8.0, 11.0),
        background_level: 0.1,
        noise_sigma: 0.05, // High noise
        ..Default::default()
    };
    let (pixels, ground_truth) = generate_star_field(&config);

    // Save input
    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_noise_input.png"),
    );

    // Background subtraction
    let background = estimate_background(&pixels, width, height, TILE_SIZE);

    let bg_subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();

    save_grayscale_png(
        &bg_subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_noise_bg_subtracted.png"),
    );

    // Apply Gaussian filter
    let sigma = fwhm_to_sigma(fwhm);
    let filtered = gaussian_convolve(&bg_subtracted, width, height, sigma);
    let filtered_display = normalize_for_display(&filtered);

    save_grayscale_png(
        &filtered_display,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_noise_filtered.png"),
    );

    // Check star detectability even with noise
    let mut star_responses: Vec<f32> = Vec::new();
    for star in &ground_truth {
        let x = star.x.round() as usize;
        let y = star.y.round() as usize;
        if x > 10 && x < width - 10 && y > 10 && y < height - 10 {
            let response = filtered[y * width + x];
            star_responses.push(response);
        }
    }

    let mean_star_response: f32 =
        star_responses.iter().sum::<f32>() / star_responses.len().max(1) as f32;

    // Compute noise level in filtered image
    let sorted: Vec<f32> = {
        let mut v = filtered.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    };
    let median = sorted[sorted.len() / 2];
    let mad: f32 = sorted.iter().map(|&v| (v - median).abs()).sum::<f32>() / sorted.len() as f32;

    println!("Mean star response: {:.6}", mean_star_response);
    println!("Filtered image median: {:.6}", median);
    println!("Filtered image MAD: {:.6}", mad);
    println!("SNR estimate: {:.1}", mean_star_response / mad);

    // Even with high noise, stars should be detectable (SNR > 3)
    assert!(
        mean_star_response > mad * 3.0,
        "Star SNR {:.1} should be > 3",
        mean_star_response / mad
    );
}
