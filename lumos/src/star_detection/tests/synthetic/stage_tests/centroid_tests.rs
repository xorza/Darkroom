//! Centroid computation stage tests.
//!
//! Tests sub-pixel centroid accuracy on synthetic stars.

use crate::common::Buffer2;
use crate::math::{Aabb, Vec2us};
use crate::star_detection::BackgroundEstimate;
use crate::star_detection::centroid::measure_star;
use crate::star_detection::config::Config;
use crate::star_detection::deblend::Region;
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

/// Test centroid accuracy on precisely placed stars.
#[test]

fn test_centroid_accuracy() {
    init_tracing();

    let width = 256;
    let height = 256;
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);

    // Create stars at known sub-pixel positions
    let test_positions = vec![
        (50.0, 50.0),     // Integer position
        (100.3, 50.2),    // Sub-pixel X
        (50.7, 100.8),    // Sub-pixel Y
        (150.5, 150.5),   // Half-pixel both
        (100.25, 100.75), // Quarter-pixel
    ];

    // Create image with uniform background
    let mut pixels = vec![0.1f32; width * height];

    // Add stars at test positions
    for &(x, y) in &test_positions {
        let amplitude = 0.8 / (2.0 * std::f32::consts::PI * sigma * sigma);
        render_gaussian_star(&mut pixels, width, x, y, sigma, amplitude);
    }

    // Add small amount of noise
    let mut rng = crate::testing::TestRng::new(42);
    for p in &mut pixels {
        let u1 = rng.next_f32().max(1e-10);
        let u2 = rng.next_f32();
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        *p += z * 0.01;
        *p = p.clamp(0.0, 1.0);
    }

    // Save input
    save_grayscale(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_centroid_accuracy_input.png"),
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

    // Test centroid computation for each star
    let mut errors = Vec::new();
    let config = Config {
        expected_fwhm: fwhm,
        ..Default::default()
    };

    for &(true_x, true_y) in &test_positions {
        // Create a fake candidate at the known position
        let peak_x = true_x.round() as usize;
        let peak_y = true_y.round() as usize;

        let candidate = Region {
            bbox: Aabb::new(
                Vec2us::new(peak_x.saturating_sub(5), peak_y.saturating_sub(5)),
                Vec2us::new((peak_x + 5).min(width - 1), (peak_y + 5).min(height - 1)),
            ),
            peak: Vec2us::new(peak_x, peak_y),
            peak_value: pixels[peak_y * width + peak_x],
            area: 50,
        };

        let result = measure_star(&pixels_buf, &background, &candidate, &config);

        if let Some(star) = result {
            let error = ((star.pos.x as f32 - true_x).powi(2)
                + (star.pos.y as f32 - true_y).powi(2))
            .sqrt();
            errors.push(error);

            println!(
                "True: ({:.2}, {:.2}) -> Detected: ({:.3}, {:.3}), error: {:.4}px",
                true_x, true_y, star.pos.x, star.pos.y, error
            );
        } else {
            println!(
                "Failed to compute centroid at ({:.2}, {:.2})",
                true_x, true_y
            );
        }
    }

    // Create overlay showing true vs detected positions
    let mut img = gray_to_rgb_image_stretched(&pixels, width, height);

    let blue = Color::rgb(0.3, 0.3, 1.0);
    let green = Color::GREEN;

    for &(true_x, true_y) in &test_positions {
        // True position in blue
        draw_circle(&mut img, Vec2::new(true_x, true_y), 8.0, blue, 1.0);

        // Detected position
        let peak_x = true_x.round() as usize;
        let peak_y = true_y.round() as usize;

        let candidate = Region {
            bbox: Aabb::new(
                Vec2us::new(peak_x.saturating_sub(5), peak_y.saturating_sub(5)),
                Vec2us::new((peak_x + 5).min(width - 1), (peak_y + 5).min(height - 1)),
            ),
            peak: Vec2us::new(peak_x, peak_y),
            peak_value: pixels[peak_y * width + peak_x],
            area: 50,
        };

        let result = measure_star(&pixels_buf, &background, &candidate, &config);

        if let Some(star) = result {
            draw_cross(
                &mut img,
                Vec2::new(star.pos.x as f32, star.pos.y as f32),
                3.0,
                green,
                1.0,
            );
        }
    }

    save_image(
        img,
        &test_output_path("synthetic_starfield/stage_centroid_accuracy_overlay.png"),
    );

    // Calculate statistics
    if !errors.is_empty() {
        let mean_error: f32 = errors.iter().sum::<f32>() / errors.len() as f32;
        let max_error = errors.iter().cloned().fold(0.0f32, f32::max);

        println!("\nCentroid accuracy:");
        println!("  Mean error: {:.4} pixels", mean_error);
        println!("  Max error: {:.4} pixels", max_error);

        // Sub-pixel accuracy should be < 0.35 pixels
        // (allowing some margin for noise and edge effects)
        assert!(
            mean_error < 0.35,
            "Mean centroid error {:.4} should be < 0.35 pixels",
            mean_error
        );
    }
}

/// Test centroid on stars with different SNR.
#[test]

fn test_centroid_snr() {
    init_tracing();

    let width = 256;
    let height = 128;
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);

    // Create stars with different brightness
    let brightnesses = [0.9, 0.5, 0.3, 0.15, 0.08];
    let mut pixels = vec![0.1f32; width * height];

    let y = 64.37; // Same sub-pixel Y for all
    let mut true_positions = Vec::new();

    for (i, &brightness) in brightnesses.iter().enumerate() {
        let x = 50.0 + i as f32 * 100.0 + 0.42; // Sub-pixel X
        true_positions.push((x, y, brightness));

        let amplitude = brightness / (2.0 * std::f32::consts::PI * sigma * sigma);
        render_gaussian_star(&mut pixels, width, x, y, sigma, amplitude);
    }

    // Add noise
    let noise_sigma = 0.02;
    let mut rng = crate::testing::TestRng::new(42);
    for p in &mut pixels {
        let u1 = rng.next_f32().max(1e-10);
        let u2 = rng.next_f32();
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        *p += z * noise_sigma;
        *p = p.clamp(0.0, 1.0);
    }

    save_grayscale(
        &pixels,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_centroid_snr_input.png"),
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

    // Create overlay
    let mut img = gray_to_rgb_image_stretched(&pixels, width, height);

    let blue = Color::rgb(0.3, 0.3, 1.0);
    let green = Color::GREEN;

    let config = Config {
        expected_fwhm: fwhm,
        ..Default::default()
    };

    println!("\nCentroid accuracy vs SNR:");
    println!("  Noise sigma: {}", noise_sigma);

    for (true_x, true_y, brightness) in &true_positions {
        let peak_x = true_x.round() as usize;
        let peak_y = true_y.round() as usize;

        draw_circle(&mut img, Vec2::new(*true_x, *true_y), 6.0, blue, 1.0);

        let candidate = Region {
            bbox: Aabb::new(
                Vec2us::new(peak_x.saturating_sub(5), peak_y.saturating_sub(5)),
                Vec2us::new((peak_x + 5).min(width - 1), (peak_y + 5).min(height - 1)),
            ),
            peak: Vec2us::new(peak_x, peak_y),
            peak_value: pixels[peak_y * width + peak_x],
            area: 50,
        };

        let result = measure_star(&pixels_buf, &background, &candidate, &config);

        if let Some(star) = result {
            let error = ((star.pos.x - *true_x as f64).powi(2)
                + (star.pos.y - *true_y as f64).powi(2))
            .sqrt();
            draw_cross(
                &mut img,
                Vec2::new(star.pos.x as f32, star.pos.y as f32),
                3.0,
                green,
                1.0,
            );

            println!(
                "  Brightness={:.2}: SNR={:.1}, error={:.4}px",
                brightness, star.snr, error
            );
        } else {
            println!("  Brightness={:.2}: Failed to compute centroid", brightness);
        }
    }

    save_image(
        img,
        &test_output_path("synthetic_starfield/stage_centroid_snr_overlay.png"),
    );
}
