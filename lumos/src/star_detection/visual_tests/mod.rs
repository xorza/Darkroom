//! Visual tests for star detection - generates debug images for inspection.

mod debug_steps;

pub mod output;
mod subpixel_accuracy;
mod synthetic;

// Algorithm stage tests
mod stage_tests;
// Pipeline tests
mod pipeline_tests;

use crate::AstroImage;

use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::init_tracing;
use image::GrayImage;
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};
use output::{gray_to_rgb_image_stretched, save_image_png};
use synthetic::{SyntheticFieldConfig, SyntheticStar, generate_star_field};

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

    // Create a simple synthetic star field
    let config = SyntheticFieldConfig {
        width: 256,
        height: 256,
        background: 0.1,
        noise_sigma: 0.02,
    };

    // Place stars at known positions
    let true_stars = vec![
        SyntheticStar::new(64.0, 64.0, 0.8, 3.0),   // bright star
        SyntheticStar::new(192.0, 64.0, 0.6, 2.5),  // medium star
        SyntheticStar::new(64.0, 192.0, 0.4, 2.0),  // dim star
        SyntheticStar::new(192.0, 192.0, 0.7, 3.5), // bright, wider star
        SyntheticStar::new(128.0, 128.0, 0.5, 2.0), // center star
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

    // Save the input image
    let input_image = to_gray_image(&pixels, config.width, config.height);
    let input_path =
        common::test_utils::test_output_path("synthetic_starfield/synthetic_input.png");
    input_image.save(&input_path).unwrap();
    println!("\nSaved input image to: {:?}", input_path);

    // Run star detection with higher SNR threshold to reject noise
    let detection_config = StarDetectionConfig {
        detection_sigma: 3.0,
        min_area: 5,
        max_area: 500,
        min_snr: 20.0, // Higher threshold to reject false positives
        ..Default::default()
    };

    let image = AstroImage::from_pixels(config.width, config.height, 1, pixels.clone());
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

    // Create output image with detections marked
    let mut output_image = gray_to_rgb_image_stretched(&pixels, config.width, config.height);

    // Draw true star positions in blue
    let blue = Color::rgb(0.0, 0.4, 1.0);
    for star in &true_stars {
        let cx = star.x;
        let cy = star.y;
        let radius = star.fwhm() * 1.5;
        draw_circle(&mut output_image, cx, cy, radius, blue, 1.0);
    }

    // Draw detected stars in green
    let green = Color::GREEN;
    for star in &detected_stars {
        let cx = star.x;
        let cy = star.y;
        draw_cross(&mut output_image, cx, cy, 3.0, green, 1.0);
        let radius = (star.fwhm * 0.5).max(3.0);
        draw_circle(&mut output_image, cx, cy, radius, green, 1.0);
    }

    let output_path =
        common::test_utils::test_output_path("synthetic_starfield/synthetic_detection.png");
    save_image_png(output_image, &output_path);
    println!("\nSaved detection result to: {:?}", output_path);
    println!("  Blue circles = true star positions");
    println!("  Green crosses/circles = detected stars");

    // Verify detection accuracy
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
                println!(
                    "  Matched star at ({:.1}, {:.1}): detected at ({:.1}, {:.1}), error={:.2}px",
                    true_star.x, true_star.y, det.x, det.y, dist
                );
            } else {
                println!(
                    "  MISSED star at ({:.1}, {:.1}): nearest detection at ({:.1}, {:.1}), dist={:.1}px",
                    true_star.x, true_star.y, det.x, det.y, dist
                );
            }
        }
    }

    println!(
        "\nDetection rate: {}/{} ({:.0}%)",
        matched,
        true_stars.len(),
        100.0 * matched as f32 / true_stars.len() as f32
    );

    // Assert we found all stars
    assert_eq!(
        matched,
        true_stars.len(),
        "Should detect all {} synthetic stars",
        true_stars.len()
    );
}
