//! Debug test that outputs intermediate steps of star detection.

use super::synthetic::{SyntheticFieldConfig, SyntheticStar, generate_star_field};
use crate::AstroImage;
use crate::astro_image::{AstroImageMetadata, ImageDimensions};
use crate::star_detection::background::estimate_background;
use crate::star_detection::constants::dilate_mask;
use crate::star_detection::{StarDetectionConfig, find_stars, median_filter_3x3};
use crate::testing::{calibration_dir, init_tracing};
use image::{GrayImage, Rgb, RgbImage};
use imageproc::drawing::{draw_cross_mut, draw_hollow_circle_mut};
use imaginarium::ColorFormat;

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

// Note: dilate_mask is now imported from crate::star_detection::constants

#[test]
fn test_debug_star_detection_steps() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let image_path = cal_dir.join("calibrated_light_500x500_stretched.tiff");

    println!("Loading image: {:?}", image_path);
    let imag_image = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed();
    let astro_image: crate::AstroImage = imag_image.convert(ColorFormat::GRAY_F32).unwrap().into();

    let width = astro_image.dimensions.width;
    let height = astro_image.dimensions.height;
    let channels = astro_image.dimensions.channels;

    println!("Image size: {}x{}, channels: {}", width, height, channels);

    // Convert to grayscale
    let grayscale: Vec<f32> = astro_image.pixels;

    // Print pixel statistics
    let min_val = grayscale.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = grayscale.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mean_val: f32 = grayscale.iter().sum::<f32>() / grayscale.len() as f32;
    println!(
        "Grayscale stats: min={:.4}, max={:.4}, mean={:.4}",
        min_val, max_val, mean_val
    );

    // Save input image (stretched) - shows Bayer pattern artifacts
    let input_img = to_gray_stretched(&grayscale, width, height);
    let path = common::test_utils::test_output_path("debug_01_input_raw.png");
    input_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Apply 3x3 median filter to remove Bayer pattern
    let smoothed = median_filter_3x3(&grayscale, width, height);
    println!("Applied 3x3 median filter to remove Bayer artifacts");

    // Save smoothed image (stretched)
    let smoothed_img = to_gray_stretched(&smoothed, width, height);
    let path = common::test_utils::test_output_path("debug_02_smoothed.png");
    smoothed_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Use default config now that median filter removes Bayer artifacts
    let config = StarDetectionConfig::default();
    let background = estimate_background(&smoothed, width, height, config.background_tile_size);

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

    // Save background-subtracted image (using smoothed data)
    let subtracted: Vec<f32> = smoothed
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &b)| p - b)
        .collect();
    let sub_img = to_gray_stretched(&subtracted, width, height);
    let path = common::test_utils::test_output_path("debug_05_subtracted.png");
    sub_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Create and save threshold mask (using smoothed data)
    let mask = create_threshold_mask(
        &smoothed,
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
    let dilated = dilate_mask(&mask, width, height, 2);
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
    let image = AstroImage {
        pixels: grayscale.clone(),
        dimensions: ImageDimensions::new(width, height, 1),
        metadata: AstroImageMetadata::default(),
    };
    let result = find_stars(&image, &config);
    let stars = result.stars;
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

#[test]
fn test_debug_synthetic_steps() {
    init_tracing();

    // Generate synthetic star field
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
    let path = common::test_utils::test_output_path("synth_debug_01_input.png");
    input_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Estimate background
    let detection_config = StarDetectionConfig::default();
    let background = estimate_background(
        &grayscale,
        width,
        height,
        detection_config.background_tile_size,
    );

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
    let path = common::test_utils::test_output_path("synth_debug_02_background.png");
    bg_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Save noise map
    let noise_img = to_gray_stretched(&background.noise, width, height);
    let path = common::test_utils::test_output_path("synth_debug_03_noise.png");
    noise_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Save background-subtracted image
    let subtracted: Vec<f32> = grayscale
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &b)| p - b)
        .collect();
    let sub_img = to_gray_stretched(&subtracted, width, height);
    let path = common::test_utils::test_output_path("synth_debug_04_subtracted.png");
    sub_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Create and save threshold mask
    let mask = create_threshold_mask(
        &grayscale,
        width,
        height,
        &background.background,
        &background.noise,
        detection_config.detection_sigma,
    );
    let mask_count = mask.iter().filter(|&&b| b).count();
    println!(
        "Threshold mask: {} pixels above threshold ({:.2}%)",
        mask_count,
        100.0 * mask_count as f32 / (width * height) as f32
    );

    let mask_img = mask_to_gray(&mask, width, height);
    let path = common::test_utils::test_output_path("synth_debug_05_threshold_mask.png");
    mask_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Dilate and save
    let dilated = dilate_mask(&mask, width, height, 2);
    let dilated_count = dilated.iter().filter(|&&b| b).count();
    println!(
        "Dilated mask: {} pixels ({:.2}%)",
        dilated_count,
        100.0 * dilated_count as f32 / (width * height) as f32
    );

    let dilated_img = mask_to_gray(&dilated, width, height);
    let path = common::test_utils::test_output_path("synth_debug_06_dilated_mask.png");
    dilated_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    // Run full detection
    let image = AstroImage {
        pixels: grayscale.clone(),
        dimensions: ImageDimensions::new(width, height, 1),
        metadata: AstroImageMetadata::default(),
    };
    let detection_result = find_stars(&image, &detection_config);
    let stars = detection_result.stars;
    println!(
        "\nDetected {} stars (expected {})",
        stars.len(),
        true_stars.len()
    );

    // Create result image with detections
    let mut result = RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let v = input_img.get_pixel(x, y).0[0];
        Rgb([v, v, v])
    });

    // Draw true positions in blue
    let blue = Rgb([0u8, 100, 255]);
    for star in &true_stars {
        let cx = star.x.round() as i32;
        let cy = star.y.round() as i32;
        let radius = (star.fwhm() * 1.2) as i32;
        draw_hollow_circle_mut(&mut result, (cx, cy), radius, blue);
    }

    // Draw detected stars in green
    let green = Rgb([0u8, 255, 0]);
    for (i, star) in stars.iter().enumerate() {
        let cx = star.x.round() as i32;
        let cy = star.y.round() as i32;
        let radius = (star.fwhm * 0.5).max(3.0) as i32;

        draw_hollow_circle_mut(&mut result, (cx, cy), radius, green);
        draw_cross_mut(&mut result, green, cx, cy);

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

    let path = common::test_utils::test_output_path("synth_debug_07_detections.png");
    result.save(&path).unwrap();
    println!("Saved: {:?}", path);
    println!("  Blue circles = true star positions");
    println!("  Green crosses/circles = detected stars");

    println!("\nAll synthetic debug images saved to test_output/");
}

#[test]
fn test_noise_analysis() {
    use crate::star_detection::estimate_background;

    let cal_dir = match std::env::var("LUMOS_CALIBRATION_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping");
            return;
        }
    };

    let cropped_path = cal_dir.join("calibrated_light_500x500.tiff");
    if !cropped_path.exists() {
        eprintln!("No cropped test image found, skipping");
        return;
    }

    let imag_image = imaginarium::Image::read_file(&cropped_path).expect("Failed to load image");
    let astro_image: crate::AstroImage = imag_image.into();

    let width = astro_image.dimensions.width;
    let height = astro_image.dimensions.height;

    // Convert to grayscale
    let grayscale = astro_image.to_grayscale().pixels;

    // Print pixel value histogram
    let min_val = grayscale.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = grayscale.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    println!("Pixel range: {:.6} to {:.6}", min_val, max_val);

    // Compute histogram
    let nbins = 100;
    let mut hist = vec![0usize; nbins];
    for &v in &grayscale {
        let bin = ((v - min_val) / (max_val - min_val) * (nbins - 1) as f32) as usize;
        let bin = bin.min(nbins - 1);
        hist[bin] += 1;
    }

    // Find peak (mode)
    let peak_bin = hist.iter().enumerate().max_by_key(|(_, c)| *c).unwrap().0;
    let peak_val = min_val + (peak_bin as f32 + 0.5) * (max_val - min_val) / nbins as f32;
    println!("Histogram peak at bin {}: value ~{:.6}", peak_bin, peak_val);

    // Estimate background with different tile sizes
    for tile_size in [32, 64, 128] {
        let bg = estimate_background(&grayscale, width, height, tile_size);

        let bg_mean = bg.background.iter().sum::<f32>() / bg.background.len() as f32;
        let noise_mean = bg.noise.iter().sum::<f32>() / bg.noise.len() as f32;
        let noise_min = bg.noise.iter().cloned().fold(f32::INFINITY, f32::min);
        let noise_max = bg.noise.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

        println!("\nTile size {}:", tile_size);
        println!("  Background mean: {:.6}", bg_mean);
        println!(
            "  Noise mean: {:.6}, min: {:.6}, max: {:.6}",
            noise_mean, noise_min, noise_max
        );

        // Count pixels above threshold for different sigma values
        for sigma in [3.0, 4.0, 5.0, 8.0, 10.0] {
            let count = grayscale
                .iter()
                .zip(bg.background.iter())
                .zip(bg.noise.iter())
                .filter(|((p, b), n)| **p > **b + sigma * n.max(1e-6))
                .count();
            println!(
                "  {} sigma: {} pixels ({:.2}%)",
                sigma,
                count,
                100.0 * count as f32 / grayscale.len() as f32
            );
        }
    }

    // Also try computing noise differently - using local pixel differences
    // This is more robust to gradients
    let mut local_diffs: Vec<f32> = Vec::new();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let idx = y * width + x;
            let c = grayscale[idx];
            // Horizontal and vertical neighbors
            let diff_h = (grayscale[idx + 1] - c).abs();
            let diff_v = (grayscale[idx + width] - c).abs();
            local_diffs.push(diff_h);
            local_diffs.push(diff_v);
        }
    }
    use crate::star_detection::constants::MAD_TO_SIGMA;
    local_diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let local_mad = local_diffs[local_diffs.len() / 2];
    let local_sigma = local_mad * MAD_TO_SIGMA / std::f32::consts::SQRT_2; // Divide by sqrt(2) for difference of 2 values
    println!(
        "\nLocal difference-based noise estimate: {:.6}",
        local_sigma
    );
}

#[test]
fn test_threshold_detail() {
    use crate::star_detection::estimate_background;

    let cal_dir = match std::env::var("LUMOS_CALIBRATION_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping");
            return;
        }
    };

    let cropped_path = cal_dir.join("calibrated_light_500x500.tiff");
    if !cropped_path.exists() {
        eprintln!("No cropped test image found, skipping");
        return;
    }

    let imag_image = imaginarium::Image::read_file(&cropped_path).expect("Failed to load image");
    let astro_image: crate::AstroImage = imag_image.into();

    let width = astro_image.dimensions.width;
    let height = astro_image.dimensions.height;

    // Convert to grayscale
    let grayscale = astro_image.to_grayscale().pixels;

    // Look at a small 20x20 region in detail
    let region_x = 100;
    let region_y = 100;
    let region_size = 20;

    println!(
        "Raw pixel values in 20x20 region at ({}, {}):",
        region_x, region_y
    );
    for y in region_y..region_y + region_size {
        let mut row = String::new();
        for x in region_x..region_x + region_size {
            let v = grayscale[y * width + x];
            row.push_str(&format!("{:.5} ", v));
        }
        println!("y={:3}: {}", y, row);
    }

    // Compute background
    let bg = estimate_background(&grayscale, width, height, 64);

    println!("\nBackground values in same region:");
    for y in region_y..region_y + region_size {
        let mut row = String::new();
        for x in region_x..region_x + region_size {
            let v = bg.background[y * width + x];
            row.push_str(&format!("{:.5} ", v));
        }
        println!("y={:3}: {}", y, row);
    }

    println!("\nNoise values in same region:");
    for y in region_y..region_y + region_size {
        let mut row = String::new();
        for x in region_x..region_x + region_size {
            let v = bg.noise[y * width + x];
            row.push_str(&format!("{:.6} ", v));
        }
        println!("y={:3}: {}", y, row);
    }

    // Show threshold (background + 8*noise)
    println!("\nThreshold (bg + 8*noise) in same region:");
    for y in region_y..region_y + region_size {
        let mut row = String::new();
        for x in region_x..region_x + region_size {
            let threshold = bg.background[y * width + x] + 8.0 * bg.noise[y * width + x];
            row.push_str(&format!("{:.5} ", threshold));
        }
        println!("y={:3}: {}", y, row);
    }

    // Show which pixels pass threshold
    println!("\nPixels above threshold (1=above, 0=below):");
    for y in region_y..region_y + region_size {
        let mut row = String::new();
        for x in region_x..region_x + region_size {
            let v = grayscale[y * width + x];
            let threshold = bg.background[y * width + x] + 8.0 * bg.noise[y * width + x];
            row.push_str(if v > threshold { "1 " } else { "0 " });
        }
        println!("y={:3}: {}", y, row);
    }
}

#[test]
fn test_find_striped_region() {
    use crate::star_detection::estimate_background;

    let cal_dir = match std::env::var("LUMOS_CALIBRATION_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping");
            return;
        }
    };

    let cropped_path = cal_dir.join("calibrated_light_500x500.tiff");
    if !cropped_path.exists() {
        eprintln!("No cropped test image found, skipping");
        return;
    }

    let imag_image = imaginarium::Image::read_file(&cropped_path).expect("Failed to load image");
    let astro_image: crate::AstroImage = imag_image.into();

    let width = astro_image.dimensions.width;
    let height = astro_image.dimensions.height;

    // Convert to grayscale
    let grayscale = astro_image.to_grayscale().pixels;

    let bg = estimate_background(&grayscale, width, height, 64);

    // Find a region with many threshold-passing pixels
    // Scan for a 30x30 region with >10 pixels above 8-sigma
    let region_size = 30;
    let mut best_region = (0, 0, 0);

    for start_y in (0..height - region_size).step_by(10) {
        for start_x in (0..width - region_size).step_by(10) {
            let mut count = 0;
            for y in start_y..start_y + region_size {
                for x in start_x..start_x + region_size {
                    let idx = y * width + x;
                    let threshold = bg.background[idx] + 8.0 * bg.noise[idx];
                    if grayscale[idx] > threshold {
                        count += 1;
                    }
                }
            }
            if count > best_region.2 && count < 200 {
                // Some pixels but not too many (avoid stars)
                best_region = (start_x, start_y, count);
            }
        }
    }

    let (region_x, region_y, count) = best_region;
    println!(
        "Found region at ({}, {}) with {} above-threshold pixels",
        region_x, region_y, count
    );

    // Print the mask pattern for this region
    println!("\nPixels above threshold (8 sigma):");
    for y in region_y..region_y + region_size {
        let mut row = String::new();
        for x in region_x..region_x + region_size {
            let idx = y * width + x;
            let threshold = bg.background[idx] + 8.0 * bg.noise[idx];
            row.push(if grayscale[idx] > threshold { '#' } else { '.' });
        }
        println!("y={:3}: {}", y, row);
    }

    // Print raw pixel values for a smaller region to see the pattern
    println!("\nRaw pixel values (multiplied by 1000 for readability) in 10x10 sub-region:");
    for y in region_y..region_y + 10 {
        let mut row = String::new();
        for x in region_x..region_x + 10 {
            let v = grayscale[y * width + x] * 1000.0;
            row.push_str(&format!("{:5.1} ", v));
        }
        println!("y={:3}: {}", y, row);
    }
}

#[test]
fn test_dilation_comparison() {
    use crate::star_detection::detection::dilate_mask;
    use crate::star_detection::estimate_background;

    let cal_dir = match std::env::var("LUMOS_CALIBRATION_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping");
            return;
        }
    };

    let cropped_path = cal_dir.join("calibrated_light_500x500.tiff");
    if !cropped_path.exists() {
        eprintln!("No cropped test image found, skipping");
        return;
    }

    let imag_image = imaginarium::Image::read_file(&cropped_path).expect("Failed to load image");
    let astro_image: crate::AstroImage = imag_image.into();

    let width = astro_image.dimensions.width;
    let height = astro_image.dimensions.height;

    // Convert to grayscale
    let grayscale = astro_image.to_grayscale().pixels;

    // Apply median filter
    let smoothed = median_filter_3x3(&grayscale, width, height);

    let bg = estimate_background(&smoothed, width, height, 64);

    // Create threshold mask
    let mask: Vec<bool> = smoothed
        .iter()
        .zip(bg.background.iter())
        .zip(bg.noise.iter())
        .map(|((&p, &b), &n)| p > b + 4.0 * n.max(1e-6))
        .collect();

    let mask_count = mask.iter().filter(|&&b| b).count();
    println!("Threshold mask: {} pixels", mask_count);

    // Count connected components WITHOUT dilation
    let (_labels_no_dil, num_no_dil) = connected_components(&mask, width, height);
    println!("Without dilation: {} connected components", num_no_dil);

    // Count connected components WITH dilation (radius 1)
    let dilated_1 = dilate_mask(&mask, width, height, 1);
    let (_labels_dil_1, num_dil_1) = connected_components(&dilated_1, width, height);
    println!("With dilation radius=1: {} connected components", num_dil_1);

    // Count connected components WITH dilation (radius 2)
    let dilated_2 = dilate_mask(&mask, width, height, 2);
    let (_labels_dil_2, num_dil_2) = connected_components(&dilated_2, width, height);
    println!("With dilation radius=2: {} connected components", num_dil_2);

    // Save both masks for visual comparison
    let no_dil_img = mask_to_gray(&mask, width, height);
    let path = common::test_utils::test_output_path("compare_no_dilation.png");
    no_dil_img.save(&path).unwrap();
    println!("Saved: {:?}", path);

    let dil_2_img = mask_to_gray(&dilated_2, width, height);
    let path = common::test_utils::test_output_path("compare_dilation_r2.png");
    dil_2_img.save(&path).unwrap();
    println!("Saved: {:?}", path);
}

fn connected_components(mask: &[bool], width: usize, height: usize) -> (Vec<u32>, usize) {
    let mut labels = vec![0u32; width * height];
    let mut parent: Vec<u32> = Vec::new();
    let mut next_label = 1u32;

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if !mask[idx] {
                continue;
            }

            let mut neighbors = Vec::with_capacity(2);

            if x > 0 && mask[idx - 1] {
                neighbors.push(labels[idx - 1]);
            }
            if y > 0 && mask[idx - width] {
                neighbors.push(labels[idx - width]);
            }

            if neighbors.is_empty() {
                labels[idx] = next_label;
                parent.push(next_label);
                next_label += 1;
            } else {
                let min_label = *neighbors.iter().min().unwrap();
                labels[idx] = min_label;
                for &label in &neighbors {
                    union(&mut parent, min_label, label);
                }
            }
        }
    }

    let mut label_map = vec![0u32; parent.len() + 1];
    let mut num_labels = 0u32;

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if labels[idx] == 0 {
                continue;
            }
            let root = find(&parent, labels[idx]);
            if label_map[root as usize] == 0 {
                num_labels += 1;
                label_map[root as usize] = num_labels;
            }
            labels[idx] = label_map[root as usize];
        }
    }

    (labels, num_labels as usize)
}

fn find(parent: &[u32], mut label: u32) -> u32 {
    while parent[(label - 1) as usize] != label {
        label = parent[(label - 1) as usize];
    }
    label
}

fn union(parent: &mut [u32], a: u32, b: u32) {
    let root_a = find(parent, a);
    let root_b = find(parent, b);
    if root_a != root_b {
        if root_a < root_b {
            parent[(root_b - 1) as usize] = root_a;
        } else {
            parent[(root_a - 1) as usize] = root_b;
        }
    }
}
