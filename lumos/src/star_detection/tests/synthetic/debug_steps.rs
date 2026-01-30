//! Debug test that outputs intermediate steps of star detection.

use super::{SyntheticFieldConfig, SyntheticStar, generate_star_field};
use crate::common::Buffer2;
use crate::star_detection::tests::common::{gray_to_rgb_image_stretched, save_image};
use crate::{AstroImage, ImageDimensions};

use crate::star_detection::background::estimate_background;
use crate::star_detection::constants::dilate_mask;
use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::init_tracing;
use image::GrayImage;
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

/// Convert f32 pixels to grayscale image (auto-stretched to full range).
fn to_gray_stretched(pixels: &[f32], width: usize, height: usize) -> GrayImage {
    let min = pixels.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = pixels.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (max - min).max(1e-10);

    let bytes: Vec<u8> = pixels
        .iter()
        .map(|&p| (((p - min) / range) * 255.0) as u8)
        .collect();

    GrayImage::from_raw(width as u32, height as u32, bytes).unwrap()
}

/// Convert bool mask to grayscale image.
fn mask_to_gray(mask: &[bool], width: usize, height: usize) -> GrayImage {
    let bytes: Vec<u8> = mask.iter().map(|&b| if b { 255 } else { 0 }).collect();
    GrayImage::from_raw(width as u32, height as u32, bytes).unwrap()
}

/// Create threshold mask (same logic as detection.rs).
fn create_threshold_mask(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
) -> Vec<bool> {
    let mut mask = vec![false; width * height];

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let bg = background[idx];
            let n = noise[idx].max(1e-6);
            let threshold = bg + sigma_threshold * n;
            mask[idx] = pixels[idx] > threshold;
        }
    }

    mask
}

#[test]
fn test_debug_synthetic_steps() {
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
    println!("  Size: {}x{}", config.width, config.height);
    println!("  Background: {}", config.background);
    println!("  Noise sigma: {}", config.noise_sigma);
    println!("  Stars: {}", true_stars.len());

    let grayscale = generate_star_field(&config, &true_stars);
    let width = config.width;
    let height = config.height;

    let min_val = grayscale.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = grayscale.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mean_val: f32 = grayscale.iter().sum::<f32>() / grayscale.len() as f32;
    println!(
        "Grayscale stats: min={:.4}, max={:.4}, mean={:.4}",
        min_val, max_val, mean_val
    );

    let input_img = to_gray_stretched(&grayscale, width, height);
    let path = common::test_utils::test_output_path("synthetic_starfield/synth_debug_01_input.png");
    input_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    let detection_config = StarDetectionConfig::default();
    let grayscale_buf = Buffer2::new(width, height, grayscale.clone());
    let background = estimate_background(&grayscale_buf, detection_config.background_tile_size);

    println!(
        "Background stats: min={:.4}, max={:.4}",
        background
            .background
            .iter()
            .cloned()
            .fold(f32::INFINITY, f32::min),
        background
            .background
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max)
    );
    println!(
        "Noise stats: min={:.6}, max={:.6}",
        background
            .noise
            .iter()
            .cloned()
            .fold(f32::INFINITY, f32::min),
        background
            .noise
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max)
    );

    let bg_img = to_gray_stretched(&background.background, width, height);
    let path =
        common::test_utils::test_output_path("synthetic_starfield/synth_debug_02_background.png");
    bg_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    let noise_img = to_gray_stretched(&background.noise, width, height);
    let path = common::test_utils::test_output_path("synthetic_starfield/synth_debug_03_noise.png");
    noise_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    let subtracted: Vec<f32> = grayscale
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &b)| p - b)
        .collect();
    let sub_img = to_gray_stretched(&subtracted, width, height);
    let path =
        common::test_utils::test_output_path("synthetic_starfield/synth_debug_04_subtracted.png");
    sub_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    let mask = create_threshold_mask(
        &grayscale,
        width,
        height,
        background.background.pixels(),
        background.noise.pixels(),
        detection_config.background_config.detection_sigma,
    );
    let mask_count = mask.iter().filter(|&&b| b).count();
    println!(
        "Threshold mask: {} pixels above threshold ({:.2}%)",
        mask_count,
        100.0 * mask_count as f32 / (width * height) as f32
    );

    let mask_img = mask_to_gray(&mask, width, height);
    let path = common::test_utils::test_output_path(
        "synthetic_starfield/synth_debug_05_threshold_mask.png",
    );
    mask_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    let mask_buf = Buffer2::new(width, height, mask.clone());
    let mut dilated_buf = Buffer2::new_filled(width, height, false);
    dilate_mask(&mask_buf, 2, &mut dilated_buf);
    let dilated = dilated_buf.into_vec();
    let dilated_count = dilated.iter().filter(|&&b| b).count();
    println!(
        "Dilated mask: {} pixels ({:.2}%)",
        dilated_count,
        100.0 * dilated_count as f32 / (width * height) as f32
    );

    let dilated_img = mask_to_gray(&dilated, width, height);
    let path =
        common::test_utils::test_output_path("synthetic_starfield/synth_debug_06_dilated_mask.png");
    dilated_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    let image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), grayscale.clone());
    let detection_result = find_stars(&image, &detection_config);
    let stars = detection_result.stars;
    println!(
        "\nDetected {} stars (expected {})",
        stars.len(),
        true_stars.len()
    );

    let mut result_img = gray_to_rgb_image_stretched(&grayscale, width, height);

    let blue = Color::rgb(0.0, 0.4, 1.0);
    for star in &true_stars {
        let cx = star.x;
        let cy = star.y;
        let radius = star.fwhm() * 1.2;
        draw_circle(&mut result_img, cx, cy, radius, blue, 1.0);
    }

    let green = Color::GREEN;
    for (i, star) in stars.iter().enumerate() {
        let cx = star.x;
        let cy = star.y;
        let radius = (star.fwhm * 0.5).max(3.0);

        draw_circle(&mut result_img, cx, cy, radius, green, 1.0);
        draw_cross(&mut result_img, cx, cy, 3.0, green, 1.0);

        println!(
            "  Detected {}: pos=({:.1}, {:.1}) fwhm={:.1} snr={:.1} flux={:.2}",
            i + 1,
            star.x,
            star.y,
            star.fwhm,
            star.snr,
            star.flux
        );
    }

    let path =
        common::test_utils::test_output_path("synthetic_starfield/synth_debug_07_detections.png");
    save_image(result_img, &path);
    println!("Saved: {:?}", path);
    println!("  Blue circles = true star positions");
    println!("  Green crosses/circles = detected stars");

    println!("\nAll synthetic debug images saved to test_output/");
}
