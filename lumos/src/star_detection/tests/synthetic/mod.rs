//! Synthetic tests for star detection algorithms.
//!
//! These tests use generated star fields to verify detection accuracy
//! without requiring real calibration data.

mod debug_steps;
mod pipeline_tests;
mod stage_tests;
mod star_field;
mod subpixel_accuracy;

pub use star_field::{SyntheticFieldConfig, SyntheticStar, generate_star_field};

use crate::star_detection::tests::common::{gray_to_rgb_image_stretched, save_image};
use crate::star_detection::{BackgroundConfig, StarDetectionConfig, find_stars};
use crate::testing::init_tracing;
use crate::{AstroImage, ImageDimensions};
use image::GrayImage;
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

/// Convert f32 grayscale pixels to u8 grayscale image.
fn to_gray_image(pixels: &[f32], width: usize, height: usize) -> GrayImage {
    let bytes: Vec<u8> = pixels
        .iter()
        .map(|&p| (p.clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    GrayImage::from_raw(width as u32, height as u32, bytes).unwrap()
}

#[test]
fn test_synthetic_star_detection() {
    init_tracing();

    let config = SyntheticFieldConfig {
        width: 256,
        height: 256,
        background: 0.1,
        noise_sigma: 0.02,
    };

    let true_stars = vec![
        SyntheticStar::new(64.0, 64.0, 0.8, 3.0),
        SyntheticStar::new(192.0, 64.0, 0.6, 2.5),
        SyntheticStar::new(64.0, 192.0, 0.4, 2.0),
        SyntheticStar::new(192.0, 192.0, 0.7, 3.5),
        SyntheticStar::new(128.0, 128.0, 0.5, 2.0),
    ];

    println!("Generating synthetic star field...");
    println!("  Image size: {}x{}", config.width, config.height);
    println!("  Background: {}", config.background);
    println!("  Noise sigma: {}", config.noise_sigma);
    println!("  Number of stars: {}", true_stars.len());

    for (i, star) in true_stars.iter().enumerate() {
        println!(
            "  Star {}: pos=({:.1}, {:.1}) brightness={:.2} sigma={:.1} fwhm={:.1}",
            i + 1,
            star.x,
            star.y,
            star.brightness,
            star.sigma,
            star.fwhm()
        );
    }

    let pixels = generate_star_field(&config, &true_stars);

    let input_image = to_gray_image(&pixels, config.width, config.height);
    let input_path =
        common::test_utils::test_output_path("synthetic_starfield/synthetic_input.png");
    input_image.save(&input_path).unwrap();
    println!("\nSaved input image to: {:?}", input_path);

    let detection_config = StarDetectionConfig {
        min_area: 5,
        max_area: 500,
        min_snr: 20.0,
        background_config: BackgroundConfig {
            detection_sigma: 3.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let image = AstroImage::from_pixels(
        ImageDimensions::new(config.width, config.height, 1),
        pixels.clone(),
    );
    let result = find_stars(&image, &detection_config);
    let detected_stars = result.stars;
    println!("\nDetected {} stars", detected_stars.len());

    for (i, star) in detected_stars.iter().enumerate() {
        println!(
            "  Detected {}: pos=({:.1}, {:.1}) flux={:.2} fwhm={:.1} snr={:.1}",
            i + 1,
            star.x,
            star.y,
            star.flux,
            star.fwhm,
            star.snr
        );
    }

    let mut output_image = gray_to_rgb_image_stretched(&pixels, config.width, config.height);

    let blue = Color::rgb(0.0, 0.4, 1.0);
    for star in &true_stars {
        draw_circle(
            &mut output_image,
            star.x,
            star.y,
            star.fwhm() * 1.5,
            blue,
            1.0,
        );
    }

    let green = Color::GREEN;
    for star in &detected_stars {
        draw_cross(&mut output_image, star.x, star.y, 3.0, green, 1.0);
        draw_circle(
            &mut output_image,
            star.x,
            star.y,
            (star.fwhm * 0.5).max(3.0),
            green,
            1.0,
        );
    }

    let output_path =
        common::test_utils::test_output_path("synthetic_starfield/synthetic_detection.png");
    save_image(output_image, &output_path);
    println!("\nSaved detection result to: {:?}", output_path);

    let mut matched = 0;
    for true_star in &true_stars {
        let closest = detected_stars.iter().min_by(|a, b| {
            let da = (a.x - true_star.x).powi(2) + (a.y - true_star.y).powi(2);
            let db = (b.x - true_star.x).powi(2) + (b.y - true_star.y).powi(2);
            da.partial_cmp(&db).unwrap()
        });

        if let Some(det) = closest {
            let dist = ((det.x - true_star.x).powi(2) + (det.y - true_star.y).powi(2)).sqrt();
            if dist < 3.0 {
                matched += 1;
            }
        }
    }

    assert_eq!(
        matched,
        true_stars.len(),
        "Should detect all {} synthetic stars",
        true_stars.len()
    );
}
