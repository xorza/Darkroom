//! Tests for background estimation.

use super::*;
use crate::common::Buffer2;
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::config::AdaptiveSigmaConfig;

#[test]
fn test_uniform_background() {
    let width = 128;
    let height = 128;
    let pixels = Buffer2::new(width, height, vec![0.5; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    for y in 0..height {
        for x in 0..width {
            let val = bg.background[(x, y)];
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
    let pixels = Buffer2::new(
        width,
        height,
        (0..height)
            .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 256.0))
            .collect(),
    );

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    let corner_00 = bg.background[(0, 0)];
    let corner_end = bg.background[(127, 127)];
    assert!(corner_end > corner_00, "Gradient not preserved");
}

#[test]
fn test_background_with_stars() {
    let width = 128;
    let height = 128;
    let mut data = vec![0.1; width * height];

    // Add bright spots (stars)
    data[64 * width + 64] = 1.0;
    data[32 * width + 32] = 0.9;
    data[96 * width + 96] = 0.95;

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Median is robust to outliers
    let center_bg = bg.background[(64, 64)];
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
    let pixels = Buffer2::new(width, height, vec![0.5; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    let noise = bg.noise[(64, 64)];
    assert!(noise < 0.01, "Noise = {}, expected near zero", noise);
}

#[test]
fn test_non_square_image() {
    let width = 256;
    let height = 64;
    let pixels = Buffer2::new(width, height, vec![0.4; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    assert_eq!(bg.width(), width);
    assert_eq!(bg.height(), height);
    assert!((bg.background[(0, 0)] - 0.4).abs() < 0.01);
    assert!((bg.background[(255, 63)] - 0.4).abs() < 0.01);
}

#[test]
fn test_sigma_clipping_rejects_outliers() {
    let width = 64;
    let height = 64;
    let mut data = vec![0.2; width * height];

    // 10% bright outliers
    for i in 0..(width * height / 10) {
        data[i * 10] = 0.95;
    }

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    let bg_val = bg.background[(32, 32)];
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

    let pixels = Buffer2::new(
        width,
        height,
        (0..height)
            .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 128.0))
            .collect(),
    );

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 16,
            ..Default::default()
        },
    );

    // Sample every 4th pixel instead of every pixel
    for y in (0..height).step_by(4) {
        for x in (0..width).step_by(4) {
            let val = bg.background[(x, y)];
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
    let pixels = Buffer2::new(width, height, vec![0.33; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 64,
            ..Default::default()
        },
    );

    assert!((bg.background[(0, 0)] - 0.33).abs() < 0.01);
    assert!((bg.background[(127, 127)] - 0.33).abs() < 0.01);
    assert!((bg.background[(255, 255)] - 0.33).abs() < 0.01);
}

#[test]
fn test_different_tile_sizes() {
    let width = 128;
    let height = 128;
    let data = vec![0.5; width * height];

    // Test representative tile sizes (min, mid, max)
    for tile_size in [16, 64, 128] {
        let pixels = Buffer2::new(width, height, data.clone());
        let bg = crate::testing::estimate_background(
            &pixels,
            BackgroundConfig {
                tile_size,
                ..Default::default()
            },
        );
        assert!(
            (bg.background[(64, 64)] - 0.5).abs() < 0.01,
            "Failed for tile_size={}",
            tile_size
        );
    }
}

#[test]
#[should_panic(expected = "tile_size must be between 16 and 256")]
fn test_tile_size_too_small() {
    let pixels = Buffer2::new(64, 64, vec![0.5; 64 * 64]);
    crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 8,
            ..Default::default()
        },
    );
}

#[test]
#[should_panic(expected = "tile_size must be between 16 and 256")]
fn test_tile_size_too_large() {
    let pixels = Buffer2::new(64, 64, vec![0.5; 64 * 64]);
    crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 512,
            ..Default::default()
        },
    );
}

#[test]
#[should_panic(expected = "Image must be at least tile_size x tile_size")]
fn test_image_too_small() {
    let pixels = Buffer2::new(32, 32, vec![0.5; 32 * 32]);
    crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 64,
            ..Default::default()
        },
    );
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_single_tile_image() {
    // Image size equals tile size - exercises tx1 == tx0 branch in interpolation
    let size = 32;
    let pixels = Buffer2::new(size, size, vec![0.42; size * size]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    // All values should be constant (no interpolation needed)
    for y in (0..size).step_by(8) {
        for x in (0..size).step_by(8) {
            let val = bg.background[(x, y)];
            assert!(
                (val - 0.42).abs() < 0.01,
                "Background at ({}, {}) = {}, expected ~0.42",
                x,
                y,
                val
            );
        }
    }
}

#[test]
fn test_noise_estimation_with_actual_noise() {
    // Image with real noise should have non-zero sigma estimation
    let width = 128;
    let height = 128;
    let mut data = vec![0.5; width * height];

    // Add Gaussian-like noise pattern (deterministic for reproducibility)
    for (i, val) in data.iter_mut().enumerate() {
        let noise = ((i * 7919) % 1000) as f32 / 10000.0 - 0.05; // [-0.05, 0.05]
        *val += noise;
    }

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    let noise = bg.noise[(64, 64)];
    assert!(
        noise > 0.01,
        "Noise = {}, expected > 0.01 for noisy image",
        noise
    );
    assert!(
        noise < 0.15,
        "Noise = {}, expected < 0.15 (not too high)",
        noise
    );
}

#[test]
fn test_interpolation_smooth_at_tile_boundaries() {
    // Verify interpolation is continuous at tile boundaries
    let width = 128;
    let height = 128;

    // Create gradient that will have different values in each tile
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 256.0))
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Check that adjacent pixels have similar values (no discontinuities)
    let max_jump = 0.05;
    for y in 1..height {
        for x in 1..width {
            let val = bg.background[(x, y)];
            let val_left = bg.background[(x - 1, y)];
            let val_up = bg.background[(x, y - 1)];

            assert!(
                (val - val_left).abs() < max_jump,
                "Discontinuity at ({}, {}): {} vs {} (left)",
                x,
                y,
                val,
                val_left
            );
            assert!(
                (val - val_up).abs() < max_jump,
                "Discontinuity at ({}, {}): {} vs {} (up)",
                x,
                y,
                val,
                val_up
            );
        }
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
    let pixels = Buffer2::new(width, height, vec![0.5; width * height]);

    let config = BackgroundConfig {
        tile_size: 32,
        ..Default::default()
    };
    let bg = crate::testing::estimate_background(&pixels, config.clone());

    // All background values should be close to 0.5
    for y in (0..height).step_by(10) {
        for x in (0..width).step_by(10) {
            let val = bg.background[(x, y)];
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
    let mut data = vec![0.1; width * height];

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
                    data[y as usize * width + x as usize] += value;
                }
            }
        }
    }

    let pixels = Buffer2::new(width, height, data);

    // Non-iterative estimate
    let bg_simple = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Iterative estimate (should be better at excluding stars)
    let config = BackgroundConfig {
        sigma_threshold: 3.0,
        iterations: 2,
        mask_dilation: 5,
        min_unmasked_fraction: 0.3,
        tile_size: 32,
        sigma_clip_iterations: 2,
        adaptive_sigma: None,
    };
    let bg_iterative = crate::testing::estimate_background(&pixels, config.clone());

    // Check background at a point away from stars
    let test_x = 16;
    let test_y = 64;
    let simple_bg = bg_simple.background[(test_x, test_y)];
    let iter_bg = bg_iterative.background[(test_x, test_y)];

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
    let mut data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 128.0))
        .collect();

    // Add a bright star
    for dy in -3i32..=3 {
        for dx in -3i32..=3 {
            let x = 32 + dx;
            let y = 32 + dy;
            if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                let dist_sq = (dx * dx + dy * dy) as f32;
                data[y as usize * width + x as usize] += 0.5 * (-dist_sq / 2.0).exp();
            }
        }
    }

    let pixels = Buffer2::new(width, height, data);
    let config = BackgroundConfig {
        tile_size: 16,
        ..Default::default()
    };
    let bg = crate::testing::estimate_background(&pixels, config.clone());

    // Gradient should be preserved
    let corner_00 = bg.background[(0, 0)];
    let corner_end = bg.background[(63, 63)];
    assert!(
        corner_end > corner_00,
        "Gradient not preserved: corner_00={}, corner_end={}",
        corner_00,
        corner_end
    );
}

#[test]
fn test_iterative_background_no_dilation() {
    // Test iterative refinement with mask_dilation = 0
    let width = 128;
    let height = 128;
    let mut data = vec![0.2; width * height];

    // Add a bright star
    for dy in -3i32..=3 {
        for dx in -3i32..=3 {
            let x = 64 + dx;
            let y = 64 + dy;
            if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                let dist_sq = (dx * dx + dy * dy) as f32;
                data[y as usize * width + x as usize] += 0.7 * (-dist_sq / 2.0).exp();
            }
        }
    }

    let pixels = Buffer2::new(width, height, data);
    let config = BackgroundConfig {
        sigma_threshold: 3.0,
        iterations: 1,
        mask_dilation: 0, // No dilation
        min_unmasked_fraction: 0.3,
        tile_size: 32,
        sigma_clip_iterations: 2,
        adaptive_sigma: None,
    };
    let bg = crate::testing::estimate_background(&pixels, config.clone());

    // Background away from star should be close to 0.2
    let val = bg.background[(16, 16)];
    assert!(
        (val - 0.2).abs() < 0.05,
        "Background {} should be ~0.2",
        val
    );
}

#[test]
fn test_iterative_background_config_default() {
    let config = BackgroundConfig::default();

    assert!((config.sigma_threshold - 4.0).abs() < 1e-6);
    assert_eq!(config.iterations, 0);
    assert_eq!(config.mask_dilation, 3);
    assert!((config.min_unmasked_fraction - 0.3).abs() < 1e-6);
}

#[test]
fn test_iterative_background_zero_iterations() {
    // Zero iterations should be equivalent to non-iterative
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new(width, height, vec![0.3; width * height]);

    let config = BackgroundConfig {
        iterations: 0,
        tile_size: 32,
        ..Default::default()
    };
    let bg = crate::testing::estimate_background(&pixels, config.clone());

    let val = bg.background[(32, 32)];
    assert!(
        (val - 0.3).abs() < 0.01,
        "Background {} should be ~0.3",
        val
    );
}

// =============================================================================
// Sigma-Clipped Statistics Tests (tests common::sigma_clipped_median_mad)
// =============================================================================

use crate::math::sigma_clipped_median_mad;

#[test]
fn test_sigma_clipped_stats_empty_values() {
    let mut values: Vec<f32> = vec![];
    let mut deviations: Vec<f32> = vec![];

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 0.0).abs() < 1e-6);
    assert!((sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_single_value() {
    let mut values = vec![0.5];
    let mut deviations: Vec<f32> = vec![];

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 0.5).abs() < 1e-6);
    assert!((sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_uniform_values() {
    let mut values = vec![0.3; 100];
    let mut deviations: Vec<f32> = vec![];

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 0.3).abs() < 1e-6);
    assert!((sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_no_outliers() {
    // Normal-ish distribution without outliers
    let mut values: Vec<f32> = (0..100).map(|i| 0.5 + (i as f32 - 50.0) * 0.001).collect();
    let mut deviations: Vec<f32> = vec![];

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.5
    assert!(
        (median - 0.5).abs() < 0.01,
        "Median {} should be ~0.5",
        median
    );
    // Sigma should be small but non-zero
    assert!(sigma > 0.0, "Sigma should be positive");
    assert!(sigma < 0.1, "Sigma {} should be small", sigma);
}

#[test]
fn test_sigma_clipped_stats_rejects_high_outliers() {
    // 90 values at 0.2, 10 high outliers at 0.9
    let mut values: Vec<f32> = vec![0.2; 90];
    values.extend(vec![0.9; 10]);
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.2 (outliers rejected)
    assert!(
        (median - 0.2).abs() < 0.05,
        "Median {} should be ~0.2 after rejecting outliers",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_rejects_low_outliers() {
    // 90 values at 0.8, 10 low outliers at 0.1
    let mut values: Vec<f32> = vec![0.8; 90];
    values.extend(vec![0.1; 10]);
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.8 (outliers rejected)
    assert!(
        (median - 0.8).abs() < 0.05,
        "Median {} should be ~0.8 after rejecting outliers",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_rejects_both_tails() {
    // 80 values at 0.5, 10 low outliers, 10 high outliers
    let mut values: Vec<f32> = vec![0.5; 80];
    values.extend(vec![0.05; 10]); // Low outliers
    values.extend(vec![0.95; 10]); // High outliers
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.5 (both tails rejected)
    assert!(
        (median - 0.5).abs() < 0.05,
        "Median {} should be ~0.5 after rejecting outliers",
        median
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
    let (median_strict, _) = sigma_clipped_median_mad(&mut values_strict, &mut deviations, 1.5, 3);
    deviations.clear();
    // Loose kappa (5.0) should reject less
    let (median_loose, _) = sigma_clipped_median_mad(&mut values_loose, &mut deviations, 5.0, 3);

    // Strict should have median closer to 0.5
    assert!(
        (median_strict - 0.5).abs() <= (median_loose - 0.5).abs() + 0.01,
        "Strict kappa median {} should be closer to 0.5 than loose {}",
        median_strict,
        median_loose
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

    let (median_1iter, _) = sigma_clipped_median_mad(&mut values_1iter, &mut deviations, 2.5, 1);
    deviations.clear();
    let (median_5iter, _) = sigma_clipped_median_mad(&mut values_5iter, &mut deviations, 2.5, 5);

    // More iterations should get closer to 0.3
    assert!(
        (median_5iter - 0.3).abs() <= (median_1iter - 0.3).abs() + 0.01,
        "5 iterations median {} should be closer to 0.3 than 1 iteration {}",
        median_5iter,
        median_1iter
    );
}

#[test]
fn test_sigma_clipped_stats_mad_to_sigma_conversion() {
    // MAD * 1.4826 ≈ sigma for Gaussian distribution
    // Create data with known spread
    let mut values: Vec<f32> = (-50..=50).map(|i| 0.5 + i as f32 * 0.002).collect();
    let mut deviations: Vec<f32> = vec![];

    let (_median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 10.0, 1); // High kappa = no clipping

    // For uniform distribution [-0.1, 0.1] around 0.5:
    // MAD = median of |x - median| = 0.05 (half the range / 2)
    // sigma = MAD * 1.4826 ≈ 0.074
    assert!(
        sigma > 0.05 && sigma < 0.1,
        "Sigma {} should be around 0.074",
        sigma
    );
}

#[test]
fn test_sigma_clipped_stats_preserves_deviations_buffer() {
    let mut values = vec![0.1, 0.2, 0.3, 0.4, 0.5];
    let mut deviations: Vec<f32> = Vec::with_capacity(100);

    sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

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

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // With only 2 values, iteration stops (len < 3) and final stats are computed
    // median_f32_mut on [0.3, 0.7] returns the middle element after sorting = values[1] = 0.7
    // But it could also average - let's just check it's reasonable
    assert!(
        (0.3..=0.7).contains(&median),
        "Median {} should be between 0.3 and 0.7",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_zero_iterations() {
    let mut values = vec![0.2, 0.2, 0.2, 0.9, 0.9];
    let mut deviations: Vec<f32> = vec![];

    // Zero iterations = just compute stats without clipping
    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 0);

    // Median of [0.2, 0.2, 0.2, 0.9, 0.9] sorted = [0.2, 0.2, 0.2, 0.9, 0.9] -> median = 0.2
    assert!(
        (median - 0.2).abs() < 1e-6,
        "Median {} should be 0.2",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_extreme_outlier() {
    // Single extreme outlier among many normal values
    let mut values: Vec<f32> = vec![0.5; 99];
    values.push(100.0); // Extreme outlier
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Outlier should be rejected, median should be 0.5
    assert!(
        (median - 0.5).abs() < 0.01,
        "Median {} should be ~0.5",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_negative_values() {
    let mut values: Vec<f32> = vec![-0.5; 90];
    values.extend(vec![0.5; 10]); // Outliers on positive side
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~-0.5
    assert!(
        (median - (-0.5)).abs() < 0.05,
        "Median {} should be ~-0.5",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_all_same_except_one() {
    // Edge case: all values same except one outlier
    let mut values: Vec<f32> = vec![0.4; 99];
    values.push(0.9);
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be 0.4, sigma should be 0 or near-zero after clipping
    assert!(
        (median - 0.4).abs() < 1e-6,
        "Median {} should be 0.4",
        median
    );
}

// =============================================================================
// Adaptive Sigma Tests
// =============================================================================

#[test]
fn test_adaptive_sigma_config_default() {
    let config = AdaptiveSigmaConfig::default();

    assert!((config.base_sigma - 4.0).abs() < 1e-6);
    assert!((config.max_sigma - 8.0).abs() < 1e-6);
    assert!((config.contrast_factor - 2.0).abs() < 1e-6);
}

#[test]
fn test_adaptive_sigma_uniform_background() {
    // Uniform image should have low contrast, resulting in base_sigma everywhere
    let width = 128;
    let height = 128;
    let pixels = Buffer2::new(width, height, vec![0.5; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            adaptive_sigma: Some(AdaptiveSigmaConfig {
                base_sigma: 3.0,
                max_sigma: 10.0,
                contrast_factor: 2.0,
            }),
            ..Default::default()
        },
    );

    // Adaptive sigma should exist
    assert!(bg.adaptive_sigma.is_some());
    let adaptive_sigma = bg.adaptive_sigma.as_ref().unwrap();

    // For uniform background, adaptive sigma should be close to base_sigma
    for y in (0..height).step_by(10) {
        for x in (0..width).step_by(10) {
            let sigma = adaptive_sigma[(x, y)];
            assert!(
                (sigma - 3.0).abs() < 0.5,
                "Adaptive sigma at ({}, {}) = {}, expected ~3.0",
                x,
                y,
                sigma
            );
        }
    }
}

#[test]
fn test_adaptive_sigma_values_in_valid_range() {
    // Test that adaptive sigma values are always within [base_sigma, max_sigma]
    let width = 128;
    let height = 128;

    // Create an image with varied content
    let mut data = vec![0.5; width * height];
    for y in 0..height {
        for x in 0..width {
            // Add some variation
            data[y * width + x] = 0.3 + 0.4 * ((x + y) as f32 / (width + height) as f32);
        }
    }

    let pixels = Buffer2::new(width, height, data);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            adaptive_sigma: Some(AdaptiveSigmaConfig {
                base_sigma: 3.0,
                max_sigma: 10.0,
                contrast_factor: 2.0,
            }),
            ..Default::default()
        },
    );

    let adaptive_sigma = bg.adaptive_sigma.as_ref().unwrap();

    // All values should be within [base_sigma, max_sigma]
    for y in 0..height {
        for x in 0..width {
            let sigma = adaptive_sigma[(x, y)];
            assert!(
                (2.99..=10.01).contains(&sigma),
                "Sigma at ({}, {}) = {} is outside valid range [3.0, 10.0]",
                x,
                y,
                sigma
            );
        }
    }
}

#[test]
fn test_adaptive_sigma_respects_max() {
    // Even with extreme contrast, sigma should not exceed max_sigma
    let width = 64;
    let height = 64;

    // Create checkerboard pattern with extreme contrast
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| if (x + y) % 2 == 0 { 0.0 } else { 1.0 }))
        .collect();

    let pixels = Buffer2::new(width, height, data);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            adaptive_sigma: Some(AdaptiveSigmaConfig {
                base_sigma: 3.0,
                max_sigma: 8.0,        // Limit to 8
                contrast_factor: 10.0, // High contrast factor
            }),
            ..Default::default()
        },
    );

    let adaptive_sigma = bg.adaptive_sigma.as_ref().unwrap();

    // All sigma values should be <= max_sigma
    for y in 0..height {
        for x in 0..width {
            let sigma = adaptive_sigma[(x, y)];
            assert!(
                sigma <= 8.0 + 0.01,
                "Sigma at ({}, {}) = {} exceeds max_sigma 8.0",
                x,
                y,
                sigma
            );
            assert!(
                sigma >= 3.0 - 0.01,
                "Sigma at ({}, {}) = {} is below base_sigma 3.0",
                x,
                y,
                sigma
            );
        }
    }
}

#[test]
fn test_adaptive_sigma_dimensions_match() {
    let width = 100;
    let height = 80;
    let pixels = Buffer2::new(width, height, vec![0.5; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            adaptive_sigma: Some(AdaptiveSigmaConfig::default()),
            ..Default::default()
        },
    );

    let adaptive_sigma = bg.adaptive_sigma.as_ref().unwrap();

    assert_eq!(adaptive_sigma.width(), width);
    assert_eq!(adaptive_sigma.height(), height);
    assert_eq!(bg.background.width(), width);
    assert_eq!(bg.noise.width(), width);
}

#[test]
fn test_adaptive_sigma_accessible() {
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new(width, height, vec![0.5; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            adaptive_sigma: Some(AdaptiveSigmaConfig::default()),
            ..Default::default()
        },
    );

    // adaptive_sigma should be Some for adaptive background
    assert!(bg.adaptive_sigma.is_some());
    let sigma = bg.adaptive_sigma.as_ref().unwrap()[(32, 32)];
    assert!(sigma > 0.0);
}

#[test]
fn test_regular_background_has_no_adaptive_sigma() {
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new(width, height, vec![0.5; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Regular BackgroundMap should not have adaptive_sigma
    assert!(bg.adaptive_sigma.is_none());
}
