//! Debug test that outputs intermediate steps of star detection.

use crate::star_detection::background::estimate_background;
use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::{calibration_dir, init_tracing};
use image::{GrayImage, Rgb, RgbImage};
use imageproc::drawing::{draw_cross_mut, draw_hollow_circle_mut};

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

/// Convert f32 pixels to grayscale without stretching (0-1 mapped to 0-255).
fn to_gray_direct(pixels: &[f32], width: usize, height: usize) -> GrayImage {
    let bytes: Vec<u8> = pixels
        .iter()
        .map(|&p| (p.clamp(0.0, 1.0) * 255.0) as u8)
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

/// Dilate mask (same logic as detection.rs).
fn dilate_mask(mask: &[bool], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let mut dilated = vec![false; width * height];

    for y in 0..height {
        for x in 0..width {
            if mask[y * width + x] {
                let y_min = y.saturating_sub(radius);
                let y_max = (y + radius).min(height - 1);
                let x_min = x.saturating_sub(radius);
                let x_max = (x + radius).min(width - 1);

                for dy in y_min..=y_max {
                    for dx in x_min..=x_max {
                        dilated[dy * width + dx] = true;
                    }
                }
            }
        }
    }

    dilated
}

#[test]
fn test_debug_star_detection_steps() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    // Look for cropped test image first, fall back to full image
    let cropped_path = cal_dir.join("calibrated_light_500x500.tiff");
    let image_path = if cropped_path.exists() {
        cropped_path
    } else {
        let full_path = cal_dir.join("calibrated_light.tiff");
        if !full_path.exists() {
            eprintln!("No test image found, skipping");
            return;
        }
        full_path
    };

    println!("Loading image: {:?}", image_path);
    let imag_image = imaginarium::Image::read_file(&image_path).expect("Failed to load image");
    let astro_image: crate::AstroImage = imag_image.into();

    let width = astro_image.dimensions.width;
    let height = astro_image.dimensions.height;
    let channels = astro_image.dimensions.channels;

    println!("Image size: {}x{}, channels: {}", width, height, channels);

    // Convert to grayscale
    let grayscale: Vec<f32> = if channels == 3 {
        (0..width * height)
            .map(|i| {
                let r = astro_image.pixels[i];
                let g = astro_image.pixels[width * height + i];
                let b = astro_image.pixels[2 * width * height + i];
                0.2126 * r + 0.7152 * g + 0.0722 * b
            })
            .collect()
    } else {
        astro_image.pixels.clone()
    };

    // Print pixel statistics
    let min_val = grayscale.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = grayscale.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mean_val: f32 = grayscale.iter().sum::<f32>() / grayscale.len() as f32;
    println!(
        "Grayscale stats: min={:.4}, max={:.4}, mean={:.4}",
        min_val, max_val, mean_val
    );

    // Save input image (stretched)
    let input_img = to_gray_stretched(&grayscale, width, height);
    let path = common::test_utils::test_output_path("debug_01_input_stretched.png");
    input_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Save input image (direct 0-1 mapping)
    let input_direct = to_gray_direct(&grayscale, width, height);
    let path = common::test_utils::test_output_path("debug_02_input_direct.png");
    input_direct.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Estimate background
    let config = StarDetectionConfig::default();
    let background = estimate_background(&grayscale, width, height, config.background_tile_size);

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

    // Save background map
    let bg_img = to_gray_stretched(&background.background, width, height);
    let path = common::test_utils::test_output_path("debug_03_background.png");
    bg_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Save noise map
    let noise_img = to_gray_stretched(&background.noise, width, height);
    let path = common::test_utils::test_output_path("debug_04_noise.png");
    noise_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Save background-subtracted image
    let subtracted: Vec<f32> = grayscale
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &b)| p - b)
        .collect();
    let sub_img = to_gray_stretched(&subtracted, width, height);
    let path = common::test_utils::test_output_path("debug_05_subtracted.png");
    sub_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Create and save threshold mask
    let mask = create_threshold_mask(
        &grayscale,
        width,
        height,
        &background.background,
        &background.noise,
        config.detection_sigma,
    );
    let mask_count = mask.iter().filter(|&&b| b).count();
    println!(
        "Threshold mask: {} pixels above threshold ({:.2}%)",
        mask_count,
        100.0 * mask_count as f32 / (width * height) as f32
    );

    let mask_img = mask_to_gray(&mask, width, height);
    let path = common::test_utils::test_output_path("debug_06_threshold_mask.png");
    mask_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Dilate and save
    let dilated = dilate_mask(&mask, width, height, 1);
    let dilated_count = dilated.iter().filter(|&&b| b).count();
    println!(
        "Dilated mask: {} pixels ({:.2}%)",
        dilated_count,
        100.0 * dilated_count as f32 / (width * height) as f32
    );

    let dilated_img = mask_to_gray(&dilated, width, height);
    let path = common::test_utils::test_output_path("debug_07_dilated_mask.png");
    dilated_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Run full detection
    let stars = find_stars(&grayscale, width, height, &config);
    println!("\nDetected {} stars", stars.len());

    // Create result image with detections
    let mut result = RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let v = input_img.get_pixel(x, y).0[0];
        Rgb([v, v, v])
    });

    let green = Rgb([0u8, 255, 0]);
    let yellow = Rgb([255u8, 255, 0]);

    for (i, star) in stars.iter().take(50).enumerate() {
        let cx = star.x.round() as i32;
        let cy = star.y.round() as i32;
        let radius = (star.fwhm * 0.7).max(5.0) as i32;

        // Use yellow for top 10, green for rest
        let color = if i < 10 { yellow } else { green };

        draw_hollow_circle_mut(&mut result, (cx, cy), radius, color);
        draw_cross_mut(&mut result, color, cx, cy);

        if i < 10 {
            println!(
                "  Star {}: pos=({:.1}, {:.1}) fwhm={:.1} snr={:.1} flux={:.2}",
                i + 1,
                star.x,
                star.y,
                star.fwhm,
                star.snr,
                star.flux
            );
        }
    }

    let path = common::test_utils::test_output_path("debug_08_detections.png");
    result.save(&path).unwrap();
    println!("Saved: {:?}", path);

    println!("\nAll debug images saved to test_output/");
}
