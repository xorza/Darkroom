//! Cosmic ray rejection stage tests.
//!
//! Tests the cosmic ray detection via sharpness filtering.

use crate::{AstroImage, ImageDimensions};

use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::synthetic_tests::Scenario;
use crate::stacking::star_detection::test_common::output::image_writer::{
    gray_to_rgb_image_stretched, save_grayscale, save_image,
};
use crate::testing::init_tracing;
use crate::testing::synthetic::artifacts::add_cosmic_rays;
use common::test_utils::test_output_path;
use glam::Vec2;
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

/// Test cosmic ray rejection on star field.
#[test]

fn test_cosmic_ray_rejection() {
    init_tracing();

    let width = 256;
    let height = 256;

    // Clean star field; cosmic rays are added manually below so we know their positions.
    let frame = Scenario {
        num_stars: 30,
        ..Default::default()
    }
    .frame();
    let ground_truth = frame.truth.sources.clone();
    let mut pixels_vec = frame.image.channel(0).pixels().to_vec();

    // Add cosmic rays manually so we know their positions
    let cr_positions = add_cosmic_rays(&mut pixels_vec, width, 15, (0.5, 1.0), 123);

    save_grayscale(
        &pixels_vec,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_cr_rejection_input.png"),
    );

    // Run detection - disable CFA filter and matched filter for synthetic images
    let detection_config = Config {
        expected_fwhm: 0.0,
        ..Default::default()
    };

    let image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), pixels_vec.clone());
    let mut detector = StarDetector::from_config(detection_config.clone()).unwrap();
    let result = detector.detect(&image);
    let stars = result.stars;

    // Create overlay
    let mut img = gray_to_rgb_image_stretched(&pixels_vec, width, height);

    // Draw cosmic ray positions in red
    let red = Color::rgb(1.0, 0.2, 0.2);
    for (x, y) in &cr_positions {
        draw_cross(&mut img, Vec2::new(*x as f32, *y as f32), 3.0, red, 1.0);
    }

    // Draw true star positions in blue
    let blue = Color::rgb(0.3, 0.3, 1.0);
    for star in &ground_truth {
        draw_circle(
            &mut img,
            Vec2::new(star.pos.x as f32, star.pos.y as f32),
            8.0,
            blue,
            1.0,
        );
    }

    // Draw detected stars in green
    let green = Color::GREEN;
    for star in &stars {
        draw_circle(
            &mut img,
            Vec2::new(star.pos.x as f32, star.pos.y as f32),
            5.0,
            green,
            1.0,
        );
    }

    save_image(
        img,
        &test_output_path("synthetic_starfield/stage_cr_rejection_overlay.png"),
    );

    // Count how many cosmic rays were falsely detected as stars
    let mut cr_false_positives = 0;
    for (cr_x, cr_y) in &cr_positions {
        for star in &stars {
            let dx = star.pos.x as f32 - *cr_x as f32;
            let dy = star.pos.y as f32 - *cr_y as f32;
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
            let dx = star.pos.x - truth.pos.x;
            let dy = star.pos.y - truth.pos.y;
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

/// Sharpness discriminates real PSF stars (low sharpness) from cosmic-ray spikes (high
/// sharpness): real stars must read below the CR threshold, and almost no CR survives as a star.
#[test]
fn detected_real_stars_have_low_sharpness() {
    init_tracing();

    let width = 256;
    let height = 256;

    let frame = Scenario {
        num_stars: 10,
        ..Default::default()
    }
    .frame();
    let ground_truth = frame.truth.sources.clone();
    let mut pixels_vec = frame.image.channel(0).pixels().to_vec();
    let cr_positions = add_cosmic_rays(&mut pixels_vec, width, 10, (0.6, 0.9), 456);

    let detection_config = Config {
        expected_fwhm: 0.0,
        ..Default::default()
    };
    let image = AstroImage::from_pixels(ImageDimensions::new((width, height), 1), pixels_vec);
    let stars = StarDetector::from_config(detection_config)
        .unwrap()
        .detect(&image)
        .stars;

    let mut real = 0;
    let mut cr_as_star = 0;
    for star in &stars {
        let is_real = ground_truth.iter().any(|t| {
            let dx = t.pos.x - star.pos.x;
            let dy = t.pos.y - star.pos.y;
            (dx * dx + dy * dy).sqrt() < 5.0
        });
        let is_cr = cr_positions.iter().any(|&(cx, cy)| {
            let dx = star.pos.x - cx as f64;
            let dy = star.pos.y - cy as f64;
            (dx * dx + dy * dy).sqrt() < 3.0
        });
        if is_real {
            real += 1;
            // The discriminator treats sharpness > 0.7 as a cosmic ray; a true PSF star must
            // sit well below that.
            assert!(
                star.sharpness < 0.7,
                "real star at ({:.1},{:.1}) has CR-like sharpness {:.3}",
                star.pos.x,
                star.pos.y,
                star.sharpness
            );
        } else if is_cr {
            cr_as_star += 1;
        }
    }
    assert!(
        real >= 8,
        "should detect most of the 10 true stars, got {real}"
    );
    assert!(
        cr_as_star <= 2,
        "cosmic rays should be rejected, {cr_as_star} survived as stars"
    );
}
