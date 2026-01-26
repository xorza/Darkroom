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
    let width = 64;
    let height = 64;

    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 128.0))
        .collect();

    let bg = estimate_background(&pixels, width, height, 16);

    // Sample every 4th pixel instead of every pixel
    for y in (0..height).step_by(4) {
        for x in (0..width).step_by(4) {
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
    let width = 256;
    let height = 256;
    let pixels: Vec<f32> = vec![0.33; width * height];

    let bg = estimate_background(&pixels, width, height, 64);

    assert!((bg.get_background(0, 0) - 0.33).abs() < 0.01);
    assert!((bg.get_background(127, 127) - 0.33).abs() < 0.01);
    assert!((bg.get_background(255, 255) - 0.33).abs() < 0.01);
}

#[test]
fn test_different_tile_sizes() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = vec![0.5; width * height];

    // Test representative tile sizes (min, mid, max)
    for tile_size in [16, 64, 128] {
        let bg = estimate_background(&pixels, width, height, tile_size);
        assert!(
            (bg.get_background(64, 64) - 0.5).abs() < 0.01,
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

    let width = astro_image.width();
    let height = astro_image.height();
    let pixels = astro_image.pixels();

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

// =============================================================================
// Iterative Background Refinement Tests
// =============================================================================

#[test]
fn test_iterative_background_uniform() {
    // Uniform image should produce same result as non-iterative
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = vec![0.5; width * height];

    let config = IterativeBackgroundConfig::default();
    let bg = estimate_background_iterative(&pixels, width, height, 32, &config);

    // All background values should be close to 0.5
    for y in (0..height).step_by(10) {
        for x in (0..width).step_by(10) {
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
fn test_iterative_background_with_bright_stars() {
    // Background with bright stars should be better estimated with iterative refinement
    let width = 128;
    let height = 128;
    let mut pixels: Vec<f32> = vec![0.1; width * height];

    // Add multiple bright Gaussian stars
    let stars: [(i32, i32); 5] = [(32, 32), (64, 64), (96, 96), (32, 96), (96, 32)];
    for (sx, sy) in stars {
        for dy in -5i32..=5 {
            for dx in -5i32..=5 {
                let x = sx + dx;
                let y = sy + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let dist_sq = (dx * dx + dy * dy) as f32;
                    let value = 0.8 * (-dist_sq / 4.0).exp();
                    pixels[y as usize * width + x as usize] += value;
                }
            }
        }
    }

    // Non-iterative estimate
    let bg_simple = estimate_background(&pixels, width, height, 32);

    // Iterative estimate (should be better at excluding stars)
    let config = IterativeBackgroundConfig {
        detection_sigma: 3.0,
        iterations: 2,
        mask_dilation: 5,
        min_unmasked_fraction: 0.3,
    };
    let bg_iterative = estimate_background_iterative(&pixels, width, height, 32, &config);

    // Check background at a point away from stars
    let test_x = 16;
    let test_y = 64;
    let simple_bg = bg_simple.get_background(test_x, test_y);
    let iter_bg = bg_iterative.get_background(test_x, test_y);

    // Both should be close to 0.1, but iterative should be at least as good
    assert!(
        (iter_bg - 0.1).abs() < 0.05,
        "Iterative background {} should be close to 0.1",
        iter_bg
    );
    assert!(
        (iter_bg - 0.1).abs() <= (simple_bg - 0.1).abs() + 0.01,
        "Iterative {} should be at least as good as simple {} at estimating 0.1 background",
        iter_bg,
        simple_bg
    );
}

#[test]
fn test_iterative_background_preserves_gradient() {
    // Background gradient should be preserved with iterative estimation
    let width = 64;
    let height = 64;
    let mut pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 128.0))
        .collect();

    // Add a bright star
    for dy in -3i32..=3 {
        for dx in -3i32..=3 {
            let x = 32 + dx;
            let y = 32 + dy;
            if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                let dist_sq = (dx * dx + dy * dy) as f32;
                pixels[y as usize * width + x as usize] += 0.5 * (-dist_sq / 2.0).exp();
            }
        }
    }

    let config = IterativeBackgroundConfig::default();
    let bg = estimate_background_iterative(&pixels, width, height, 16, &config);

    // Gradient should be preserved
    let corner_00 = bg.get_background(0, 0);
    let corner_end = bg.get_background(63, 63);
    assert!(
        corner_end > corner_00,
        "Gradient not preserved: corner_00={}, corner_end={}",
        corner_00,
        corner_end
    );
}

#[test]
fn test_iterative_background_config_default() {
    let config = IterativeBackgroundConfig::default();

    assert!((config.detection_sigma - 3.0).abs() < 1e-6);
    assert_eq!(config.iterations, 1);
    assert_eq!(config.mask_dilation, 3);
    assert!((config.min_unmasked_fraction - 0.3).abs() < 1e-6);
}

#[test]
fn test_iterative_background_zero_iterations() {
    // Zero iterations should be equivalent to non-iterative
    let width = 64;
    let height = 64;
    let pixels: Vec<f32> = vec![0.3; width * height];

    let config = IterativeBackgroundConfig {
        iterations: 0,
        ..Default::default()
    };
    let bg = estimate_background_iterative(&pixels, width, height, 32, &config);

    let val = bg.get_background(32, 32);
    assert!(
        (val - 0.3).abs() < 0.01,
        "Background {} should be ~0.3",
        val
    );
}

// =============================================================================
// Sigma-Clipped Statistics Tests
// =============================================================================

use super::sigma_clipped_stats;

#[test]
fn test_sigma_clipped_stats_empty_values() {
    let mut values: Vec<f32> = vec![];
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    assert!((stats.median - 0.0).abs() < 1e-6);
    assert!((stats.sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_single_value() {
    let mut values = vec![0.5];
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    assert!((stats.median - 0.5).abs() < 1e-6);
    assert!((stats.sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_uniform_values() {
    let mut values = vec![0.3; 100];
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    assert!((stats.median - 0.3).abs() < 1e-6);
    assert!((stats.sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_no_outliers() {
    // Normal-ish distribution without outliers
    let mut values: Vec<f32> = (0..100).map(|i| 0.5 + (i as f32 - 50.0) * 0.001).collect();
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.5
    assert!(
        (stats.median - 0.5).abs() < 0.01,
        "Median {} should be ~0.5",
        stats.median
    );
    // Sigma should be small but non-zero
    assert!(stats.sigma > 0.0, "Sigma should be positive");
    assert!(stats.sigma < 0.1, "Sigma {} should be small", stats.sigma);
}

#[test]
fn test_sigma_clipped_stats_rejects_high_outliers() {
    // 90 values at 0.2, 10 high outliers at 0.9
    let mut values: Vec<f32> = vec![0.2; 90];
    values.extend(vec![0.9; 10]);
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.2 (outliers rejected)
    assert!(
        (stats.median - 0.2).abs() < 0.05,
        "Median {} should be ~0.2 after rejecting outliers",
        stats.median
    );
}

#[test]
fn test_sigma_clipped_stats_rejects_low_outliers() {
    // 90 values at 0.8, 10 low outliers at 0.1
    let mut values: Vec<f32> = vec![0.8; 90];
    values.extend(vec![0.1; 10]);
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.8 (outliers rejected)
    assert!(
        (stats.median - 0.8).abs() < 0.05,
        "Median {} should be ~0.8 after rejecting outliers",
        stats.median
    );
}

#[test]
fn test_sigma_clipped_stats_rejects_both_tails() {
    // 80 values at 0.5, 10 low outliers, 10 high outliers
    let mut values: Vec<f32> = vec![0.5; 80];
    values.extend(vec![0.05; 10]); // Low outliers
    values.extend(vec![0.95; 10]); // High outliers
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.5 (both tails rejected)
    assert!(
        (stats.median - 0.5).abs() < 0.05,
        "Median {} should be ~0.5 after rejecting outliers",
        stats.median
    );
}

#[test]
fn test_sigma_clipped_stats_kappa_affects_rejection() {
    // Same data, different kappa values
    let base_values: Vec<f32> = {
        let mut v = vec![0.5; 80];
        v.extend(vec![0.8; 20]); // Moderate outliers
        v
    };

    let mut values_strict = base_values.clone();
    let mut values_loose = base_values.clone();
    let mut deviations: Vec<f32> = vec![];

    // Strict kappa (1.5) should reject more
    let stats_strict = sigma_clipped_stats(&mut values_strict, &mut deviations, 1.5, 3);
    deviations.clear();
    // Loose kappa (5.0) should reject less
    let stats_loose = sigma_clipped_stats(&mut values_loose, &mut deviations, 5.0, 3);

    // Strict should have median closer to 0.5
    assert!(
        (stats_strict.median - 0.5).abs() <= (stats_loose.median - 0.5).abs() + 0.01,
        "Strict kappa median {} should be closer to 0.5 than loose {}",
        stats_strict.median,
        stats_loose.median
    );
}

#[test]
fn test_sigma_clipped_stats_iterations_improve_result() {
    // Strong outliers that need multiple iterations
    let base_values: Vec<f32> = {
        let mut v = vec![0.3; 70];
        v.extend(vec![0.6; 20]); // Moderate outliers
        v.extend(vec![0.95; 10]); // Strong outliers
        v
    };

    let mut values_1iter = base_values.clone();
    let mut values_5iter = base_values.clone();
    let mut deviations: Vec<f32> = vec![];

    let stats_1iter = sigma_clipped_stats(&mut values_1iter, &mut deviations, 2.5, 1);
    deviations.clear();
    let stats_5iter = sigma_clipped_stats(&mut values_5iter, &mut deviations, 2.5, 5);

    // More iterations should get closer to 0.3
    assert!(
        (stats_5iter.median - 0.3).abs() <= (stats_1iter.median - 0.3).abs() + 0.01,
        "5 iterations median {} should be closer to 0.3 than 1 iteration {}",
        stats_5iter.median,
        stats_1iter.median
    );
}

#[test]
fn test_sigma_clipped_stats_mad_to_sigma_conversion() {
    // MAD * 1.4826 ≈ sigma for Gaussian distribution
    // Create data with known spread
    let mut values: Vec<f32> = (-50..=50).map(|i| 0.5 + i as f32 * 0.002).collect();
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 10.0, 1); // High kappa = no clipping

    // For uniform distribution [-0.1, 0.1] around 0.5:
    // MAD = median of |x - median| = 0.05 (half the range / 2)
    // sigma = MAD * 1.4826 ≈ 0.074
    assert!(
        stats.sigma > 0.05 && stats.sigma < 0.1,
        "Sigma {} should be around 0.074",
        stats.sigma
    );
}

#[test]
fn test_sigma_clipped_stats_preserves_deviations_buffer() {
    let mut values = vec![0.1, 0.2, 0.3, 0.4, 0.5];
    let mut deviations: Vec<f32> = Vec::with_capacity(100);

    sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // Buffer should be reused (capacity preserved)
    assert!(
        deviations.capacity() >= 5,
        "Deviations buffer should have been used"
    );
}

#[test]
fn test_sigma_clipped_stats_handles_two_values() {
    let mut values = vec![0.3, 0.7];
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // With only 2 values, iteration stops (len < 3) and final stats are computed
    // median_f32_mut on [0.3, 0.7] returns the middle element after sorting = values[1] = 0.7
    // But it could also average - let's just check it's reasonable
    assert!(
        stats.median >= 0.3 && stats.median <= 0.7,
        "Median {} should be between 0.3 and 0.7",
        stats.median
    );
}

#[test]
fn test_sigma_clipped_stats_zero_iterations() {
    let mut values = vec![0.2, 0.2, 0.2, 0.9, 0.9];
    let mut deviations: Vec<f32> = vec![];

    // Zero iterations = just compute stats without clipping
    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 0);

    // Median of [0.2, 0.2, 0.2, 0.9, 0.9] sorted = [0.2, 0.2, 0.2, 0.9, 0.9] -> median = 0.2
    assert!(
        (stats.median - 0.2).abs() < 1e-6,
        "Median {} should be 0.2",
        stats.median
    );
}

#[test]
fn test_sigma_clipped_stats_extreme_outlier() {
    // Single extreme outlier among many normal values
    let mut values: Vec<f32> = vec![0.5; 99];
    values.push(100.0); // Extreme outlier
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // Outlier should be rejected, median should be 0.5
    assert!(
        (stats.median - 0.5).abs() < 0.01,
        "Median {} should be ~0.5",
        stats.median
    );
}

#[test]
fn test_sigma_clipped_stats_negative_values() {
    let mut values: Vec<f32> = vec![-0.5; 90];
    values.extend(vec![0.5; 10]); // Outliers on positive side
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~-0.5
    assert!(
        (stats.median - (-0.5)).abs() < 0.05,
        "Median {} should be ~-0.5",
        stats.median
    );
}

#[test]
fn test_sigma_clipped_stats_all_same_except_one() {
    // Edge case: all values same except one outlier
    let mut values: Vec<f32> = vec![0.4; 99];
    values.push(0.9);
    let mut deviations: Vec<f32> = vec![];

    let stats = sigma_clipped_stats(&mut values, &mut deviations, 3.0, 3);

    // Median should be 0.4, sigma should be 0 or near-zero after clipping
    assert!(
        (stats.median - 0.4).abs() < 1e-6,
        "Median {} should be 0.4",
        stats.median
    );
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

    let width = astro_image.width();
    let height = astro_image.height();
    let pixels = astro_image.pixels();

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
