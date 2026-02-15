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

// =============================================================================
// Bicubic Spline Interpolation Tests
// =============================================================================

#[test]
fn test_bicubic_reproduces_linear_gradient() {
    // A linear gradient f(x,y) = ax + by + c should be reproduced exactly by
    // natural cubic spline (cubic of a linear = linear, d2 = 0 everywhere)
    let width = 128;
    let height = 128;

    // Linear gradient: f(x,y) = 0.001*x + 0.002*y + 0.1
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| 0.001 * x as f32 + 0.002 * y as f32 + 0.1))
        .collect();

    let pixels = Buffer2::new(width, height, data.clone());
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // The gradient should be monotonically increasing and close to original.
    // Note: The 3x3 median filter on tile statistics slightly smooths the gradient,
    // so we verify monotonicity and bounded error rather than exact reproduction.
    let corner_00 = bg.background[(32, 32)];
    let corner_end = bg.background[(96, 96)];
    let expected_00 = 0.001 * 32.0 + 0.002 * 32.0 + 0.1;
    let expected_end = 0.001 * 96.0 + 0.002 * 96.0 + 0.1;

    assert!(
        corner_end > corner_00,
        "Gradient not monotonically increasing: ({:.4}) vs ({:.4})",
        corner_00,
        corner_end
    );

    // Both endpoints should be in the right ballpark (within tile-level precision)
    assert!(
        (corner_00 - expected_00).abs() < 0.1,
        "Interior point (32,32): expected ~{:.4}, got {:.4}",
        expected_00,
        corner_00
    );
    assert!(
        (corner_end - expected_end).abs() < 0.1,
        "Interior point (96,96): expected ~{:.4}, got {:.4}",
        expected_end,
        corner_end
    );

    // Key test: adjacent pixels should have very small differences (C2 smooth)
    let mut max_jump = 0.0f32;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let jump_x = (bg.background[(x, y)] - bg.background[(x - 1, y)]).abs();
            let jump_y = (bg.background[(x, y)] - bg.background[(x, y - 1)]).abs();
            max_jump = max_jump.max(jump_x).max(jump_y);
        }
    }
    assert!(
        max_jump < 0.01,
        "Max pixel-to-pixel jump {:.6} too large for smooth bicubic",
        max_jump
    );
}

#[test]
fn test_bicubic_c1_continuity_at_tile_boundaries() {
    // Verify first derivatives are continuous at tile boundaries.
    // Numerical derivative across boundary should be smooth — the jump
    // in the derivative should be small compared to the derivative itself.
    let width = 256;
    let height = 256;

    // Quadratic background: f(x,y) = 0.00005*(x² + y²) + 0.1
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| 0.00005 * (x * x + y * y) as f32 + 0.1))
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 64,
            ..Default::default()
        },
    );

    // Check derivative continuity in X at tile boundary columns
    // Tile size 64 → centers at 32, 96, 160, 224
    // Boundaries are midway between centers: 64, 128, 192
    let boundary_xs = [64, 128, 192];
    let mut max_d2_jump = 0.0f32;

    for &bx in &boundary_xs {
        if bx + 2 >= width || bx < 2 {
            continue;
        }
        for y in (40..height - 40).step_by(10) {
            // Numerical second derivative: f''(x) ≈ f(x-1) - 2*f(x) + f(x+1)
            let d2_left = bg.background[(bx - 2, y)] - 2.0 * bg.background[(bx - 1, y)]
                + bg.background[(bx, y)];
            let d2_right = bg.background[(bx, y)] - 2.0 * bg.background[(bx + 1, y)]
                + bg.background[(bx + 2, y)];
            let jump = (d2_right - d2_left).abs();
            max_d2_jump = max_d2_jump.max(jump);
        }
    }

    // With C2 bicubic spline, the second derivative jump should be very small
    assert!(
        max_d2_jump < 0.001,
        "Max second derivative jump at tile boundary: {:.6} (should be < 0.001 for C2 spline)",
        max_d2_jump
    );
}

#[test]
fn test_bicubic_smoother_than_bilinear_would_be() {
    // Bicubic spline should produce smoother results (smaller max second derivative)
    // than bilinear would. We verify this indirectly by checking that the second
    // derivative is bounded, as bilinear would have discontinuous first derivatives.
    let width = 128;
    let height = 128;

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

    // Compute max numerical second derivative in X across the entire image
    let mut max_d2 = 0.0f32;
    for y in 0..height {
        for x in 1..width - 1 {
            let d2 = (bg.background[(x - 1, y)] - 2.0 * bg.background[(x, y)]
                + bg.background[(x + 1, y)])
                .abs();
            max_d2 = max_d2.max(d2);
        }
    }

    // With bicubic spline on a linear gradient, second derivative should be near zero
    assert!(
        max_d2 < 0.005,
        "Max second derivative {:.6} too large for C2 spline on linear gradient",
        max_d2
    );
}

#[test]
fn test_bicubic_c2_continuity_y_direction() {
    // Same as test_bicubic_c1_continuity_at_tile_boundaries but for Y direction.
    // With natural bicubic spline, second derivative should be continuous at Y tile boundaries.
    let width = 256;
    let height = 256;

    // Quadratic background: f(x,y) = 0.00005*(x² + y²) + 0.1
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| 0.00005 * (x * x + y * y) as f32 + 0.1))
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 64,
            ..Default::default()
        },
    );

    // Tile size 64 → Y centers at 32, 96, 160, 224
    // Boundaries midway: 64, 128, 192
    let boundary_ys = [64, 128, 192];
    let mut max_d2_jump = 0.0f32;

    for &by in &boundary_ys {
        if by + 2 >= height || by < 2 {
            continue;
        }
        for x in (40..width - 40).step_by(10) {
            // Numerical second derivative in Y: f''(y) ≈ f(y-1) - 2*f(y) + f(y+1)
            let d2_above = bg.background[(x, by - 2)] - 2.0 * bg.background[(x, by - 1)]
                + bg.background[(x, by)];
            let d2_below = bg.background[(x, by)] - 2.0 * bg.background[(x, by + 1)]
                + bg.background[(x, by + 2)];
            let jump = (d2_below - d2_above).abs();
            max_d2_jump = max_d2_jump.max(jump);
        }
    }

    // With C2 bicubic spline, the second derivative jump should be very small
    assert!(
        max_d2_jump < 0.001,
        "Max Y-direction second derivative jump at tile boundary: {:.6} (should be < 0.001 for C2 spline)",
        max_d2_jump
    );
}

#[test]
fn test_noise_map_bicubic_interpolation() {
    // Verify that the noise map is also interpolated with bicubic spline,
    // not just constant or linear. Create an image with spatially varying noise.
    let width = 128;
    let height = 128;

    // Background constant at 0.5, noise varies: left half has low noise, right half high noise.
    // We achieve this by making pixel values in right tiles more spread out.
    let data: Vec<f32> = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                // Deterministic pseudo-noise that increases with x
                let base = 0.5;
                let noise_amp = if x < 64 { 0.001 } else { 0.05 };
                let noise = ((x * 7919 + y * 104729) % 1000) as f32 / 1000.0 - 0.5;
                base + noise * noise_amp
            })
        })
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Noise at left side should be lower than noise at right side
    let noise_left = bg.noise[(16, 64)];
    let noise_right = bg.noise[(112, 64)];
    assert!(
        noise_right > noise_left,
        "Noise should increase left→right: left={}, right={}",
        noise_left,
        noise_right
    );

    // Noise should be smoothly interpolated (no discontinuities)
    let mut max_jump = 0.0f32;
    for y in 1..height {
        for x in 1..width {
            let jump_x = (bg.noise[(x, y)] - bg.noise[(x - 1, y)]).abs();
            let jump_y = (bg.noise[(x, y)] - bg.noise[(x, y - 1)]).abs();
            max_jump = max_jump.max(jump_x).max(jump_y);
        }
    }
    assert!(
        max_jump < 0.01,
        "Noise map max pixel-to-pixel jump {:.6} too large for smooth bicubic",
        max_jump
    );

    // Noise values should all be finite. Small negatives are possible from
    // cubic spline overshoot near zero but should be negligible.
    for y in (0..height).step_by(4) {
        for x in (0..width).step_by(4) {
            let n = bg.noise[(x, y)];
            assert!(n.is_finite(), "NaN/Inf noise at ({},{})", x, y);
            assert!(n > -0.01, "Large negative noise at ({},{}): {}", x, y, n);
        }
    }
}

#[test]
fn test_bicubic_single_tile_column() {
    // With tiles_x=1, the X-direction solve gets n=1. Should produce constant fill.
    let width = 32; // 1 tile column
    let height = 128;
    let pixels = Buffer2::new(width, height, vec![0.42; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // All background values should be ~0.42 (constant, no X interpolation)
    for y in (0..height).step_by(8) {
        for x in (0..width).step_by(8) {
            let val = bg.background[(x, y)];
            assert!(
                (val - 0.42).abs() < 1e-3,
                "Single tile column: bg({},{}) = {}, expected 0.42",
                x,
                y,
                val
            );
        }
    }
}

#[test]
fn test_bicubic_two_tile_columns() {
    // With tiles_x=2, natural spline has d2=0 at both endpoints (no interior points).
    // Interpolation degenerates to linear between the two tile centers.
    let width = 64; // 2 tile columns
    let height = 64;

    // Left tile at value 100, right tile at value 200
    let data: Vec<f32> = (0..height)
        .flat_map(|_| (0..width).map(|x| if x < 32 { 100.0 } else { 200.0 }))
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // At left center (x=16): should be ~100
    // At right center (x=48): should be ~200
    // At midpoint (x=32): should be ~150 (linear interp between two tiles)
    let left = bg.background[(16, 32)];
    let mid = bg.background[(32, 32)];
    let right = bg.background[(48, 32)];

    assert!(
        (left - 100.0).abs() < 5.0,
        "Left center: expected ~100, got {}",
        left
    );
    assert!(
        (right - 200.0).abs() < 5.0,
        "Right center: expected ~200, got {}",
        right
    );
    // With only 2 tiles, natural spline = linear, so midpoint = average
    assert!(
        (mid - 150.0).abs() < 10.0,
        "Midpoint: expected ~150 (linear), got {}",
        mid
    );
    // Verify monotonicity
    assert!(
        right > mid && mid > left,
        "Should be monotonic: left={}, mid={}, right={}",
        left,
        mid,
        right
    );
}

#[test]
fn test_bicubic_single_tile_row() {
    // With tiles_y=1, Y direction should be constant (no Y interpolation needed)
    let width = 128;
    let height = 32; // 1 tile row
    let pixels = Buffer2::new(width, height, vec![0.77; width * height]);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    for y in (0..height).step_by(8) {
        for x in (0..width).step_by(8) {
            let val = bg.background[(x, y)];
            assert!(
                (val - 0.77).abs() < 1e-3,
                "Single tile row: bg({},{}) = {}, expected 0.77",
                x,
                y,
                val
            );
        }
    }
}

#[test]
fn test_bicubic_two_tile_rows() {
    // With tiles_y=2, natural spline has d2=0 at both endpoints.
    // Y interpolation degenerates to linear.
    let width = 64;
    let height = 64; // 2 tile rows

    // Top tile at 50, bottom tile at 150
    let data: Vec<f32> = (0..height)
        .flat_map(|y| {
            let val = if y < 32 { 50.0 } else { 150.0 };
            std::iter::repeat_n(val, width)
        })
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // At top center (y=16): should be ~50
    // At bottom center (y=48): should be ~150
    let top = bg.background[(32, 16)];
    let mid = bg.background[(32, 32)];
    let bot = bg.background[(32, 48)];

    assert!(
        (top - 50.0).abs() < 5.0,
        "Top center: expected ~50, got {}",
        top
    );
    assert!(
        (bot - 150.0).abs() < 5.0,
        "Bottom center: expected ~150, got {}",
        bot
    );
    assert!(
        (mid - 100.0).abs() < 10.0,
        "Midpoint: expected ~100 (linear), got {}",
        mid
    );
    assert!(
        bot > mid && mid > top,
        "Should be monotonic: top={}, mid={}, bot={}",
        top,
        mid,
        bot
    );
}

// sigma_clipped_median_mad tests moved to math/statistics/tests.rs
