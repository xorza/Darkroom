//! Cosmic ray rejection stage tests.
//!
//! Tests the cosmic ray detection via Laplacian SNR filtering.

use crate::AstroImage;
use crate::astro_image::{AstroImageMetadata, ImageDimensions};
use crate::star_detection::visual_tests::generators::{
    StarFieldConfig, add_cosmic_rays, generate_star_field,
};
use crate::star_detection::visual_tests::output::save_grayscale_png;
use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::init_tracing;
use common::test_utils::test_output_path;
use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_cross_mut, draw_hollow_circle_mut};

/// Test cosmic ray rejection on star field.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_cosmic_ray_rejection() {
    init_tracing();

    let width = 512;
    let height = 512;

    // Create star field without cosmic rays first
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 30,
        fwhm_range: (3.5, 4.5),
        // Use proper magnitude range to avoid saturation
        magnitude_range: (12.5, 13.5),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        cosmic_ray_count: 0,
        ..Default::default()
    };
    let (mut pixels, ground_truth) = generate_star_field(&config);

    // Add cosmic rays manually so we know their positions
    let cr_positions = add_cosmic_rays(&mut pixels, width, 30, (0.5, 1.0), 123);

    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("stage_cr_rejection_input.png"),
    );

    // Run detection - disable CFA filter and matched filter for synthetic images
    let detection_config = StarDetectionConfig {
        is_cfa: false,
        expected_fwhm: 0.0,
        ..Default::default()
    };

    let image = AstroImage {
        pixels: pixels.clone(),
        dimensions: ImageDimensions::new(width, height, 1),
        metadata: AstroImageMetadata::default(),
    };
    let result = find_stars(&image, &detection_config);
    let stars = result.stars;

    // Create overlay
    let mut img = RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let idx = y as usize * width + x as usize;
        let v = (pixels[idx].clamp(0.0, 1.0) * 255.0) as u8;
        Rgb([v, v, v])
    });

    // Draw cosmic ray positions in red
    let red = Rgb([255u8, 50, 50]);
    for (x, y) in &cr_positions {
        draw_cross_mut(&mut img, red, *x as i32, *y as i32);
    }

    // Draw true star positions in blue
    let blue = Rgb([80u8, 80, 255]);
    for star in &ground_truth {
        draw_hollow_circle_mut(&mut img, (star.x as i32, star.y as i32), 8, blue);
    }

    // Draw detected stars in green
    let green = Rgb([0u8, 255, 0]);
    for star in &stars {
        draw_hollow_circle_mut(&mut img, (star.x as i32, star.y as i32), 5, green);
    }

    img.save(test_output_path("stage_cr_rejection_overlay.png"))
        .unwrap();

    // Count how many cosmic rays were falsely detected as stars
    let mut cr_false_positives = 0;
    for (cr_x, cr_y) in &cr_positions {
        for star in &stars {
            let dx = star.x - *cr_x as f32;
            let dy = star.y - *cr_y as f32;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < 3.0 {
                cr_false_positives += 1;
                break;
            }
        }
    }

    // Count how many true stars were detected
    let mut true_detections = 0;
    for truth in &ground_truth {
        for star in &stars {
            let dx = star.x - truth.x;
            let dy = star.y - truth.y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < 5.0 {
                true_detections += 1;
                break;
            }
        }
    }

    let cr_rejection_rate = 1.0 - (cr_false_positives as f32 / cr_positions.len() as f32);
    let detection_rate = true_detections as f32 / ground_truth.len() as f32;

    println!("\nCosmic Ray Rejection Results:");
    println!("  True stars: {}", ground_truth.len());
    println!("  Cosmic rays: {}", cr_positions.len());
    println!("  Detected stars: {}", stars.len());
    println!("  True detections: {}", true_detections);
    println!("  CR false positives: {}", cr_false_positives);
    println!("  CR rejection rate: {:.1}%", cr_rejection_rate * 100.0);
    println!("  Star detection rate: {:.1}%", detection_rate * 100.0);

    // Most cosmic rays should be rejected
    assert!(
        cr_rejection_rate > 0.7,
        "CR rejection rate {:.1}% should be > 70%",
        cr_rejection_rate * 100.0
    );

    // Most stars should still be detected
    assert!(
        detection_rate > 0.9,
        "Star detection rate {:.1}% should be > 90%",
        detection_rate * 100.0
    );
}

/// Test Laplacian SNR visualization.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_laplacian_snr_visualization() {
    init_tracing();

    let width = 256;
    let height = 256;

    // Create simple field with stars and cosmic rays
    let config = StarFieldConfig {
        width,
        height,
        num_stars: 10,
        fwhm_range: (4.0, 4.0),
        // Use proper magnitude range to avoid saturation
        magnitude_range: (12.5, 13.5),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        cosmic_ray_count: 0,
        ..Default::default()
    };
    let (mut pixels, ground_truth) = generate_star_field(&config);

    // Add cosmic rays
    let cr_positions = add_cosmic_rays(&mut pixels, width, 10, (0.6, 0.9), 456);

    save_grayscale_png(
        &pixels,
        width,
        height,
        &test_output_path("stage_laplacian_snr_input.png"),
    );

    // Run detection - disable CFA filter and matched filter for synthetic images
    let detection_config = StarDetectionConfig {
        is_cfa: false,
        expected_fwhm: 0.0,
        ..Default::default()
    };

    let image = AstroImage {
        pixels: pixels.clone(),
        dimensions: ImageDimensions::new(width, height, 1),
        metadata: AstroImageMetadata::default(),
    };
    let result = find_stars(&image, &detection_config);
    let stars = result.stars;

    println!("\nLaplacian SNR Analysis:");
    println!("Stars (should have low Laplacian SNR):");
    for (i, star) in stars.iter().enumerate() {
        // Check if this star matches a ground truth star
        let is_real = ground_truth.iter().any(|t| {
            let dx = t.x - star.x;
            let dy = t.y - star.y;
            (dx * dx + dy * dy).sqrt() < 5.0
        });

        let label = if is_real { "STAR" } else { "CR?" };
        println!(
            "  {}: ({:.1}, {:.1}) FWHM={:.2} SNR={:.1} Lap_SNR={:.1} sharp={:.3} [{}]",
            i, star.x, star.y, star.fwhm, star.snr, star.laplacian_snr, star.sharpness, label
        );
    }

    // Create composite visualization
    let mut img = RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let idx = y as usize * width + x as usize;
        let v = (pixels[idx].clamp(0.0, 1.0) * 255.0) as u8;
        Rgb([v, v, v])
    });

    // Mark CR positions
    let red = Rgb([255u8, 100, 100]);
    for (x, y) in &cr_positions {
        draw_cross_mut(&mut img, red, *x as i32, *y as i32);
    }

    // Mark detected stars colored by Laplacian SNR
    for star in &stars {
        // High Laplacian SNR = likely cosmic ray (red), low = likely star (green)
        let color = if star.laplacian_snr > 5.0 {
            Rgb([255u8, 0, 0]) // High Lap_SNR: likely CR
        } else if star.laplacian_snr > 2.0 {
            Rgb([255u8, 255, 0]) // Medium: uncertain
        } else {
            Rgb([0u8, 255, 0]) // Low: likely real star
        };
        draw_hollow_circle_mut(&mut img, (star.x as i32, star.y as i32), 6, color);
    }

    img.save(test_output_path("stage_laplacian_snr_overlay.png"))
        .unwrap();

    println!("\nColor legend:");
    println!("  Green circles: Low Laplacian SNR (likely real star)");
    println!("  Yellow circles: Medium Laplacian SNR (uncertain)");
    println!("  Red circles: High Laplacian SNR (likely cosmic ray)");
    println!("  Red crosses: True cosmic ray positions");
}
