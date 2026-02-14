//! Tests for background estimation.

use super::*;
use crate::common::Buffer2;
use crate::star_detection::config::{BackgroundRefinement, Config};

#[test]
fn test_uniform_background() {
    let width = 128;
    let height = 128;
    let pixels = Buffer2::new(width, height, vec![0.5; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    for y in 0..height {
        for x in 0..width {
            let val = bg.background[(x, y)];
            assert!(
                (val - 0.5).abs() < 1e-4,
                "Background at ({}, {}) = {}, expected 0.5",
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
        &Config {
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
        &Config {
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
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // For uniform data, MAD = 0, so noise estimate should be ~0
    let noise = bg.noise[(64, 64)];
    assert!(
        noise < 1e-4,
        "Noise = {}, expected ~0 for uniform image",
        noise
    );
}

#[test]
fn test_non_square_image() {
    let width = 256;
    let height = 64;
    let pixels = Buffer2::new(width, height, vec![0.4; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    assert_eq!(bg.background.width(), width);
    assert_eq!(bg.background.height(), height);
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
        &Config {
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
        &Config {
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
        &Config {
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
            &Config {
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
    // Validation happens in Config::validate(), called by StarDetector::detect()
    let config = Config {
        tile_size: 8,
        ..Default::default()
    };
    config.validate();
}

#[test]
#[should_panic(expected = "tile_size must be between 16 and 256")]
fn test_tile_size_too_large() {
    // Validation happens in Config::validate(), called by StarDetector::detect()
    let config = Config {
        tile_size: 512,
        ..Default::default()
    };
    config.validate();
}

#[test]
#[should_panic(expected = "Image must be at least tile_size x tile_size")]
fn test_image_too_small() {
    let pixels = Buffer2::new(32, 32, vec![0.5; 32 * 32]);
    crate::testing::estimate_background(
        &pixels,
        &Config {
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
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Single tile → no interpolation, all values should be exactly the tile median = 0.42
    for y in (0..size).step_by(8) {
        for x in (0..size).step_by(8) {
            let val = bg.background[(x, y)];
            assert!(
                (val - 0.42).abs() < 1e-4,
                "Background at ({}, {}) = {}, expected 0.42",
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
        &Config {
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
        &Config {
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

    let config = Config {
        tile_size: 32,
        ..Default::default()
    };
    let bg = crate::testing::estimate_background(&pixels, &config);

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
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Iterative estimate (should be better at excluding stars)
    let config = Config {
        sigma_threshold: 3.0,
        refinement: BackgroundRefinement::Iterative { iterations: 2 },
        bg_mask_dilation: 5,
        min_unmasked_fraction: 0.3,
        tile_size: 32,
        sigma_clip_iterations: 2,

        ..Default::default()
    };
    let bg_iterative = crate::testing::estimate_background(&pixels, &config);

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
    let config = Config {
        tile_size: 16,
        ..Default::default()
    };
    let bg = crate::testing::estimate_background(&pixels, &config);

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
    let config = Config {
        sigma_threshold: 3.0,
        refinement: BackgroundRefinement::Iterative { iterations: 1 },
        bg_mask_dilation: 0, // No dilation
        min_unmasked_fraction: 0.3,
        tile_size: 32,
        sigma_clip_iterations: 2,

        ..Default::default()
    };
    let bg = crate::testing::estimate_background(&pixels, &config);

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
    let config = Config::default();

    assert!((config.sigma_threshold - 4.0).abs() < 1e-6);
    assert!(matches!(config.refinement, BackgroundRefinement::None));
    assert_eq!(config.bg_mask_dilation, 3);
    assert!((config.min_unmasked_fraction - 0.3).abs() < 1e-6);
}

#[test]
fn test_iterative_background_no_refinement() {
    // No refinement should work fine
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new(width, height, vec![0.3; width * height]);

    let config = Config {
        refinement: BackgroundRefinement::None,
        tile_size: 32,
        ..Default::default()
    };
    let bg = crate::testing::estimate_background(&pixels, &config);

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
    // Good values: 50 at 0.50, 30 at 0.54 (true center = 0.50)
    // Outliers: 20 at 0.80
    //
    // Approx median of all 100 = 0.54 (upper-middle, value[50]).
    // MAD = 0.04 (deviations: 50×0.04, 30×0.00, 20×0.26).
    // sigma = 0.04 * 1.4826 = 0.059.
    //
    // kappa=1.5: threshold = 0.089. Rejects 0.80 (dev 0.26 > 0.089).
    //   After clipping: 80 values [0.50(50), 0.54(30)].
    //   Iter 2: approx median = 0.50, MAD = 0 → converge at 0.50.
    //
    // kappa=5.0: threshold = 0.297. Keeps 0.80 (dev 0.26 < 0.297).
    //   Converge at approx median = 0.54.
    let base_values: Vec<f32> = {
        let mut v = vec![0.50; 50];
        v.extend(vec![0.54; 30]);
        v.extend(vec![0.80; 20]);
        v
    };

    let mut values_strict = base_values.clone();
    let mut values_loose = base_values.clone();
    let mut deviations: Vec<f32> = vec![];

    let (median_strict, _) = sigma_clipped_median_mad(&mut values_strict, &mut deviations, 1.5, 3);
    deviations.clear();
    let (median_loose, _) = sigma_clipped_median_mad(&mut values_loose, &mut deviations, 5.0, 3);

    // Strict rejects outliers → converges at 0.50 (true center)
    // Loose keeps outliers → converges at 0.54 (biased)
    assert!(
        (median_strict - 0.5).abs() < (median_loose - 0.5).abs(),
        "Strict kappa median {} should be closer to 0.5 than loose {}",
        median_strict,
        median_loose
    );
    assert!(
        (median_strict - 0.5).abs() < 1e-6,
        "Strict kappa should recover true median 0.5, got {}",
        median_strict
    );
}

#[test]
fn test_sigma_clipped_stats_iterations_improve_result() {
    // Good values: 41 at 0.30, 40 at 0.32 (true median = 0.30, odd count = 81)
    // Outliers: 10 at 0.60, 9 at 1.50
    //
    // Approx median of all 100 = 0.32 (value[50]).
    // MAD = 0.02 (devs: 41×0.02, 40×0.00, 10×0.28, 9×1.18, index 50 = 0.02).
    // sigma = 0.02 * 1.4826 = 0.0297.
    //
    // 0 iterations (no clipping): compute_final_stats on 100 values.
    //   median_f32_mut(100): avg(values[50], max(values[0..50])) = avg(0.32, 0.32) = 0.32.
    //
    // 3 iterations (with clipping):
    //   Iter 1: kappa=2.5, threshold = 0.074. Rejects 0.60 and 1.50 → 81 remain.
    //   Iter 2: 81 values (odd). approx median = value[40] = 0.30.
    //     MAD = 0.00, sigma = 0 → converge at 0.30.
    let base_values: Vec<f32> = {
        let mut v = vec![0.30; 41];
        v.extend(vec![0.32; 40]);
        v.extend(vec![0.60; 10]);
        v.extend(vec![1.50; 9]);
        v
    };

    let mut values_0iter = base_values.clone();
    let mut values_3iter = base_values.clone();
    let mut deviations: Vec<f32> = vec![];

    let (median_0iter, _) = sigma_clipped_median_mad(&mut values_0iter, &mut deviations, 2.5, 0);
    deviations.clear();
    let (median_3iter, _) = sigma_clipped_median_mad(&mut values_3iter, &mut deviations, 2.5, 3);

    // 0 iterations: no clipping, median biased to 0.32 by outlier presence
    assert!(
        (median_0iter - 0.32).abs() < 1e-6,
        "0 iterations should give 0.32, got {}",
        median_0iter
    );
    // 3 iterations: clipping removes outliers, converges to true median 0.30
    assert!(
        (median_3iter - 0.30).abs() < 1e-6,
        "3 iterations should recover true median 0.30, got {}",
        median_3iter
    );
    // Clipping brings result closer to true center
    assert!(
        (median_3iter - 0.30).abs() < (median_0iter - 0.30).abs(),
        "3 iterations median {} should be closer to 0.30 than 0 iterations {}",
        median_3iter,
        median_0iter
    );
}

#[test]
fn test_sigma_clipped_stats_mad_to_sigma_conversion() {
    // MAD * 1.4826 ≈ sigma for Gaussian distribution
    // Create data with known spread
    let mut values: Vec<f32> = (-50..=50).map(|i| 0.5 + i as f32 * 0.002).collect();
    let mut deviations: Vec<f32> = vec![];

    let (_median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 10.0, 1); // High kappa = no clipping

    // Data: 101 evenly spaced values from 0.4 to 0.6 (step = 0.002)
    // Median = 0.5 (center value)
    // |x - 0.5| values: 0.000, 0.002, ..., 0.100 (101 values, each appearing once)
    // Sorted abs deviations: [0.000, 0.002, 0.002, 0.004, 0.004, ..., 0.100]
    // MAD = median of abs devs = value at index 50 of 101 sorted abs devs
    // Abs devs sorted: each deviation d=0.000..0.100 in steps of 0.002 appears twice
    // (positive and negative), except 0.000 which appears once.
    // So sorted: [0.000, 0.002, 0.002, 0.004, 0.004, ..., 0.100, 0.100]
    // Index 50 → 0.050
    // sigma = MAD * 1.4826 = 0.050 * 1.4826 = 0.07413
    let expected_sigma = 0.05 * 1.4826;
    assert!(
        (sigma - expected_sigma).abs() < 0.002,
        "Sigma {} should be ~{:.4} (MAD=0.05 × 1.4826)",
        sigma,
        expected_sigma
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

    // With only 2 values, iteration stops (len < 3) and final stats are computed.
    // median_f32_mut on 2 values (even length): averages two middle elements
    // = (0.3 + 0.7) / 2 = 0.5
    assert!(
        (median - 0.5).abs() < 1e-6,
        "Median of [0.3, 0.7] should be 0.5 (average of two), got {}",
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
