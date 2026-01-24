//! Tests for background estimation.

use super::*;

#[test]
fn test_uniform_background() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = vec![0.5; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    for y in 0..height {
        for x in 0..width {
            let val = bg.get_background(x, y);
            assert!(
                (val - 0.5).abs() < 0.01,
                "Background at ({}, {}) = {}, expected ~0.5",
                x,
                y,
                val
            );
        }
    }
}

#[test]
fn test_gradient_background() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 256.0))
        .collect();

    let bg = estimate_background(&pixels, width, height, 32);

    let corner_00 = bg.get_background(0, 0);
    let corner_end = bg.get_background(127, 127);
    assert!(corner_end > corner_00, "Gradient not preserved");
}

#[test]
fn test_background_with_stars() {
    let width = 128;
    let height = 128;
    let mut pixels: Vec<f32> = vec![0.1; width * height];

    // Add bright spots (stars)
    pixels[64 * width + 64] = 1.0;
    pixels[32 * width + 32] = 0.9;
    pixels[96 * width + 96] = 0.95;

    let bg = estimate_background(&pixels, width, height, 32);

    // Median is robust to outliers
    let center_bg = bg.get_background(64, 64);
    assert!(
        (center_bg - 0.1).abs() < 0.05,
        "Background at star = {}, expected ~0.1",
        center_bg
    );
}

#[test]
fn test_noise_estimation() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = vec![0.5; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    let noise = bg.get_noise(64, 64);
    assert!(noise < 0.01, "Noise = {}, expected near zero", noise);
}

#[test]
fn test_subtract_method() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = vec![0.3; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    let subtracted = bg.subtract(&pixels, 64, 64);
    assert!(
        subtracted.abs() < 0.01,
        "Subtracted = {}, expected ~0",
        subtracted
    );

    // Test with bright pixel
    let mut bright = pixels.clone();
    bright[64 * width + 64] = 0.8;
    let sub_bright = bg.subtract(&bright, 64, 64);
    assert!(
        (sub_bright - 0.5).abs() < 0.05,
        "Subtracted bright = {}, expected ~0.5",
        sub_bright
    );
}

#[test]
fn test_non_square_image() {
    let width = 256;
    let height = 64;
    let pixels: Vec<f32> = vec![0.4; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    assert_eq!(bg.width, width);
    assert_eq!(bg.height, height);
    assert!((bg.get_background(0, 0) - 0.4).abs() < 0.01);
    assert!((bg.get_background(255, 63) - 0.4).abs() < 0.01);
}

#[test]
fn test_sigma_clipping_rejects_outliers() {
    let width = 64;
    let height = 64;
    let mut pixels: Vec<f32> = vec![0.2; width * height];

    // 10% bright outliers
    for i in 0..(width * height / 10) {
        pixels[i * 10] = 0.95;
    }

    let bg = estimate_background(&pixels, width, height, 32);

    let bg_val = bg.get_background(32, 32);
    assert!(
        (bg_val - 0.2).abs() < 0.05,
        "Background = {}, expected ~0.2",
        bg_val
    );
}

#[test]
fn test_interpolation_produces_valid_values() {
    // Verify interpolation produces continuous (no NaN/Inf) values
    let width = 128;
    let height = 128;

    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 256.0))
        .collect();

    let bg = estimate_background(&pixels, width, height, 32);

    for y in 0..height {
        for x in 0..width {
            let val = bg.get_background(x, y);
            assert!(val.is_finite(), "NaN/Inf at ({},{})", x, y);
            assert!(
                (0.0..=1.0).contains(&val),
                "Out of range at ({},{}): {}",
                x,
                y,
                val
            );
        }
    }
}

#[test]
fn test_large_image() {
    let width = 512;
    let height = 512;
    let pixels: Vec<f32> = vec![0.33; width * height];

    let bg = estimate_background(&pixels, width, height, 64);

    assert!((bg.get_background(0, 0) - 0.33).abs() < 0.01);
    assert!((bg.get_background(255, 255) - 0.33).abs() < 0.01);
    assert!((bg.get_background(511, 511) - 0.33).abs() < 0.01);
}

#[test]
fn test_different_tile_sizes() {
    let width = 256;
    let height = 256;
    let pixels: Vec<f32> = vec![0.5; width * height];

    // Test min and max tile sizes
    for tile_size in [16, 32, 64, 128, 256] {
        let bg = estimate_background(&pixels, width, height, tile_size);
        assert!(
            (bg.get_background(128, 128) - 0.5).abs() < 0.01,
            "Failed for tile_size={}",
            tile_size
        );
    }
}

#[test]
#[should_panic(expected = "Tile size must be between 16 and 256")]
fn test_tile_size_too_small() {
    let pixels: Vec<f32> = vec![0.5; 64 * 64];
    estimate_background(&pixels, 64, 64, 8);
}

#[test]
#[should_panic(expected = "Tile size must be between 16 and 256")]
fn test_tile_size_too_large() {
    let pixels: Vec<f32> = vec![0.5; 64 * 64];
    estimate_background(&pixels, 64, 64, 512);
}

#[test]
#[should_panic(expected = "Image must be at least tile_size x tile_size")]
fn test_image_too_small() {
    let pixels: Vec<f32> = vec![0.5; 32 * 32];
    estimate_background(&pixels, 32, 32, 64);
}

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_background_on_real_image() {
    use crate::testing::calibration_dir;
    use image::GrayImage;
    use imaginarium::ColorFormat;

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let image_path = cal_dir.join("calibrated_light_500x500_stretched.tiff");
    if !image_path.exists() {
        eprintln!("calibrated_light_500x500_stretched.tiff not found, skipping");
        return;
    }

    // Load image
    let imag_image = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed();
    let astro_image: crate::AstroImage = imag_image.convert(ColorFormat::GRAY_F32).unwrap().into();

    let width = astro_image.dimensions.width;
    let height = astro_image.dimensions.height;
    let pixels = &astro_image.pixels;

    println!("Loaded image: {}x{}", width, height);

    // Estimate background
    let bg = estimate_background(pixels, width, height, 64);

    // Save background map
    let bg_img = to_gray_image(&bg.background, width, height);
    let path = common::test_utils::test_output_path("background_map.tiff");
    bg_img.save(&path).unwrap();
    println!("Saved background map: {:?}", path);

    // Save noise map (auto-scaled for visibility)
    let noise_min = bg.noise.iter().cloned().fold(f32::INFINITY, f32::min);
    let noise_max = bg.noise.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let noise_range = (noise_max - noise_min).max(f32::EPSILON);
    let noise_scaled: Vec<f32> = bg
        .noise
        .iter()
        .map(|n| (n - noise_min) / noise_range)
        .collect();
    let noise_img = to_gray_image(&noise_scaled, width, height);
    let path = common::test_utils::test_output_path("background_noise.tiff");
    noise_img.save(&path).unwrap();
    println!("Saved noise map: {:?}", path);
    println!("Noise range: min={:.6}, max={:.6}", noise_min, noise_max);

    // Save background-subtracted image
    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(bg.background.iter())
        .map(|(p, b)| (p - b + 0.5).clamp(0.0, 1.0)) // Shift to show negative values
        .collect();
    let sub_img = to_gray_image(&subtracted, width, height);
    let path = common::test_utils::test_output_path("background_subtracted.png");
    sub_img.save(&path).unwrap();
    println!("Saved subtracted image: {:?}", path);

    // Print statistics
    let bg_min = bg.background.iter().cloned().fold(f32::INFINITY, f32::min);
    let bg_max = bg
        .background
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);
    let bg_mean: f32 = bg.background.iter().sum::<f32>() / bg.background.len() as f32;
    println!(
        "Background stats: min={:.4}, max={:.4}, mean={:.4}",
        bg_min, bg_max, bg_mean
    );

    let noise_mean: f32 = bg.noise.iter().sum::<f32>() / bg.noise.len() as f32;
    println!("Noise mean: {:.6}", noise_mean);

    /// Convert f32 pixels to grayscale image (assumes 0-1 range).
    fn to_gray_image(pixels: &[f32], width: usize, height: usize) -> GrayImage {
        GrayImage::from_fn(width as u32, height as u32, |x, y| {
            let val = pixels[y as usize * width + x as usize];
            image::Luma([(val.clamp(0.0, 1.0) * 255.0) as u8])
        })
    }
}

#[test]
fn test_background_regression() {
    use image::GrayImage;
    use imaginarium::ColorFormat;

    let test_resources = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test_resources");

    let image_path = test_resources.join("calibrated_light_500x500_stretched.tiff");
    if !image_path.exists() {
        panic!(
            "Test resource not found: {:?}. Please add calibrated_light_500x500_stretched.tiff to test_resources/",
            image_path
        );
    }

    // Load input image
    let imag_image = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed();
    let astro_image: crate::AstroImage = imag_image.convert(ColorFormat::GRAY_F32).unwrap().into();

    let width = astro_image.dimensions.width;
    let height = astro_image.dimensions.height;
    let pixels = &astro_image.pixels;

    // Estimate background
    let bg = estimate_background(pixels, width, height, 64);

    // Load reference images
    let ref_bg_path = test_resources.join("background_map.tiff");
    let ref_noise_path = test_resources.join("background_noise.tiff");

    assert!(
        ref_bg_path.exists(),
        "Reference background_map.tiff not found in test_resources/"
    );
    assert!(
        ref_noise_path.exists(),
        "Reference background_noise.tiff not found in test_resources/"
    );

    let ref_bg_img = image::open(&ref_bg_path)
        .expect("Failed to load reference background map")
        .into_luma8();
    let ref_noise_img = image::open(&ref_noise_path)
        .expect("Failed to load reference noise map")
        .into_luma8();

    // Generate current output as grayscale images
    let bg_img = to_gray_image(&bg.background, width, height);

    // Auto-scale noise the same way as in the visual test
    let noise_min = bg.noise.iter().cloned().fold(f32::INFINITY, f32::min);
    let noise_max = bg.noise.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let noise_range = (noise_max - noise_min).max(f32::EPSILON);
    let noise_scaled: Vec<f32> = bg
        .noise
        .iter()
        .map(|n| (n - noise_min) / noise_range)
        .collect();
    let noise_img = to_gray_image(&noise_scaled, width, height);

    // Compare dimensions
    assert_eq!(
        (bg_img.width(), bg_img.height()),
        (ref_bg_img.width(), ref_bg_img.height()),
        "Background map dimensions mismatch"
    );
    assert_eq!(
        (noise_img.width(), noise_img.height()),
        (ref_noise_img.width(), ref_noise_img.height()),
        "Noise map dimensions mismatch"
    );

    // Compare pixel values with tolerance (allow small differences due to floating point)
    const MAX_DIFF: u8 = 2; // Allow up to 2 levels difference per pixel

    let mut bg_diff_count = 0;
    let mut bg_max_diff: u8 = 0;
    for (current, reference) in bg_img.pixels().zip(ref_bg_img.pixels()) {
        let diff = (current.0[0] as i16 - reference.0[0] as i16).unsigned_abs() as u8;
        if diff > MAX_DIFF {
            bg_diff_count += 1;
        }
        bg_max_diff = bg_max_diff.max(diff);
    }

    let mut noise_diff_count = 0;
    let mut noise_max_diff: u8 = 0;
    for (current, reference) in noise_img.pixels().zip(ref_noise_img.pixels()) {
        let diff = (current.0[0] as i16 - reference.0[0] as i16).unsigned_abs() as u8;
        if diff > MAX_DIFF {
            noise_diff_count += 1;
        }
        noise_max_diff = noise_max_diff.max(diff);
    }

    let total_pixels = width * height;
    let bg_diff_pct = bg_diff_count as f64 / total_pixels as f64 * 100.0;
    let noise_diff_pct = noise_diff_count as f64 / total_pixels as f64 * 100.0;

    // Allow at most 0.1% of pixels to differ by more than MAX_DIFF
    const MAX_DIFF_PCT: f64 = 0.1;

    assert!(
        bg_diff_pct <= MAX_DIFF_PCT,
        "Background map regression: {:.2}% pixels differ by more than {} (max diff: {})",
        bg_diff_pct,
        MAX_DIFF,
        bg_max_diff
    );

    assert!(
        noise_diff_pct <= MAX_DIFF_PCT,
        "Noise map regression: {:.2}% pixels differ by more than {} (max diff: {})",
        noise_diff_pct,
        MAX_DIFF,
        noise_max_diff
    );

    println!(
        "Background map: max diff = {}, pixels > {} diff = {} ({:.4}%)",
        bg_max_diff, MAX_DIFF, bg_diff_count, bg_diff_pct
    );
    println!(
        "Noise map: max diff = {}, pixels > {} diff = {} ({:.4}%)",
        noise_max_diff, MAX_DIFF, noise_diff_count, noise_diff_pct
    );

    /// Convert f32 pixels to grayscale image (assumes 0-1 range).
    fn to_gray_image(pixels: &[f32], width: usize, height: usize) -> GrayImage {
        GrayImage::from_fn(width as u32, height as u32, |x, y| {
            let val = pixels[y as usize * width + x as usize];
            image::Luma([(val.clamp(0.0, 1.0) * 255.0) as u8])
        })
    }
}
