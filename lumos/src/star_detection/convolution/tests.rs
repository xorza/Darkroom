//! Tests for Gaussian convolution.

use super::*;
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;

// ============================================================================
// Kernel generation tests
// ============================================================================

#[test]
fn test_gaussian_kernel_1d_normalization() {
    for sigma in [0.5, 1.0, 2.0, 3.0, 5.0] {
        let kernel = gaussian_kernel_1d(sigma);
        let sum: f32 = kernel.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "Kernel should sum to 1.0, got {} for sigma={}",
            sum,
            sigma
        );
    }
}

#[test]
fn test_gaussian_kernel_1d_symmetry() {
    let kernel = gaussian_kernel_1d(2.0);
    let n = kernel.len();
    for i in 0..n / 2 {
        assert!(
            (kernel[i] - kernel[n - 1 - i]).abs() < 1e-6,
            "Kernel should be symmetric"
        );
    }
}

#[test]
fn test_gaussian_kernel_1d_peak_at_center() {
    let kernel = gaussian_kernel_1d(2.0);
    let center = kernel.len() / 2;
    for (i, &v) in kernel.iter().enumerate() {
        if i != center {
            assert!(v < kernel[center], "Center should have maximum value");
        }
    }
}

#[test]
fn test_gaussian_kernel_1d_size() {
    // Kernel radius should be ceil(3 * sigma)
    let sigma = 2.0;
    let kernel = gaussian_kernel_1d(sigma);
    let expected_radius = (3.0 * sigma).ceil() as usize;
    let expected_size = 2 * expected_radius + 1;
    assert_eq!(kernel.len(), expected_size);
}

#[test]
fn test_gaussian_kernel_1d_small_sigma() {
    let kernel = gaussian_kernel_1d(0.5);
    // For sigma=0.5, radius=2, size=5
    assert_eq!(kernel.len(), 5);
    assert!(kernel[2] > 0.5, "Center should dominate for small sigma");
}

#[test]
#[should_panic(expected = "Sigma must be positive")]
fn test_gaussian_kernel_1d_zero_sigma_panics() {
    gaussian_kernel_1d(0.0);
}

#[test]
#[should_panic(expected = "Sigma must be positive")]
fn test_gaussian_kernel_1d_negative_sigma_panics() {
    gaussian_kernel_1d(-1.0);
}

// ============================================================================
// Convolution tests
// ============================================================================

#[test]
fn test_gaussian_convolve_uniform_image() {
    // Convolving a uniform image should return the same uniform value
    let width = 32;
    let height = 32;
    let pixels = Buffer2::new(width, height, vec![0.5f32; width * height]);

    let mut result = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result);

    for v in result.iter() {
        assert!(
            (v - 0.5).abs() < 1e-5,
            "Uniform image should stay uniform after convolution"
        );
    }
}

#[test]
fn test_gaussian_convolve_preserves_total_flux() {
    // Total flux should be approximately preserved (with some edge effects)
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.0f32; width * height];

    // Add a point source in the center
    pixels[32 * width + 32] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result);

    let input_sum: f32 = pixels.iter().sum();
    let output_sum: f32 = result.iter().sum();

    assert!(
        (output_sum - input_sum).abs() < 0.01,
        "Total flux should be preserved: input={}, output={}",
        input_sum,
        output_sum
    );
}

#[test]
fn test_gaussian_convolve_spreads_point_source() {
    let width = 32;
    let height = 32;
    let mut pixels = vec![0.0f32; width * height];

    // Point source at center
    let cx = 16;
    let cy = 16;
    pixels[cy * width + cx] = 1.0;

    let sigma = 2.0;
    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, sigma, &mut result);

    // Peak should be at center but reduced
    let peak = result[cy * width + cx];
    assert!(peak < 1.0, "Peak should be reduced after convolution");
    assert!(peak > 0.01, "Peak should still be significant");

    // Neighbors should have non-zero values
    assert!(result[cy * width + cx + 1] > 0.0);
    assert!(result[cy * width + cx - 1] > 0.0);
    assert!(result[(cy + 1) * width + cx] > 0.0);
    assert!(result[(cy - 1) * width + cx] > 0.0);
}

#[test]
fn test_gaussian_convolve_symmetry() {
    let width = 33;
    let height = 33;
    let mut pixels = vec![0.0f32; width * height];

    // Point source at center
    pixels[16 * width + 16] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result);

    // Check symmetry around center
    for dy in 1..8 {
        for dx in 1..8 {
            let v1 = result[(16 + dy) * width + (16 + dx)];
            let v2 = result[(16 - dy) * width + (16 + dx)];
            let v3 = result[(16 + dy) * width + (16 - dx)];
            let v4 = result[(16 - dy) * width + (16 - dx)];

            assert!((v1 - v2).abs() < 1e-6, "Should be symmetric vertically");
            assert!((v1 - v3).abs() < 1e-6, "Should be symmetric horizontally");
            assert!((v1 - v4).abs() < 1e-6, "Should be symmetric diagonally");
        }
    }
}

#[test]
fn test_gaussian_convolve_larger_sigma_more_spread() {
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.0f32; width * height];
    pixels[32 * width + 32] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result_small = Buffer2::new_default(width, height);
    let mut result_large = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 1.0, &mut result_small);
    gaussian_convolve(&pixels, 3.0, &mut result_large);

    // Larger sigma should result in lower peak
    let peak_small = result_small[32 * width + 32];
    let peak_large = result_large[32 * width + 32];

    assert!(
        peak_large < peak_small,
        "Larger sigma should spread more: small={}, large={}",
        peak_small,
        peak_large
    );
}

#[test]
fn test_gaussian_convolve_edge_handling() {
    let width = 16;
    let height = 16;
    let mut pixels = vec![0.0f32; width * height];

    // Point source near edge
    pixels[2 * width + 2] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 1.5, &mut result);

    // Should not have NaN or Inf
    for v in result.iter() {
        assert!(v.is_finite(), "All values should be finite");
    }

    // Corner should have some value due to mirror boundary
    assert!(result[0] > 0.0, "Corner should receive some flux");
}

#[test]
fn test_gaussian_convolve_non_square_image() {
    let width = 64;
    let height = 32;
    let mut pixels = vec![0.0f32; width * height];
    pixels[16 * width + 32] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result);

    assert_eq!(result.len(), width * height);

    // Check it worked
    let peak = result[16 * width + 32];
    assert!(peak > 0.0 && peak < 1.0);
}

#[test]
fn test_gaussian_convolve_small_image() {
    // Test with image smaller than typical kernel
    let width = 8;
    let height = 8;
    let pixels = Buffer2::new(width, height, vec![1.0f32; width * height]);

    let mut result = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result);

    // Should still work and preserve value approximately
    for v in result.iter() {
        assert!(v.is_finite());
        assert!((*v - 1.0).abs() < 0.1, "Uniform should stay ~uniform");
    }
}

// ============================================================================
// Matched filter tests
// ============================================================================

#[test]
fn test_matched_filter_subtracts_background() {
    let width = 32;
    let height = 32;
    let background = Buffer2::new(width, height, vec![0.3f32; width * height]);
    let pixels = Buffer2::new(width, height, vec![0.3f32; width * height]); // Same as background

    let mut result = Buffer2::new_default(width, height);
    let mut scratch = Buffer2::new_default(width, height);
    matched_filter(
        &pixels,
        &background,
        3.0,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
    );

    // Result should be near zero
    for v in result.iter() {
        assert!(v.abs() < 1e-5, "Flat field at background should give ~0");
    }
}

#[test]
fn test_matched_filter_detects_star() {
    let width = 32;
    let height = 32;
    let background = Buffer2::new(width, height, vec![0.1f32; width * height]);
    let mut pixels = vec![0.1f32; width * height];

    // Add a star
    let cx = 16;
    let cy = 16;
    pixels[cy * width + cx] = 0.5;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    let mut scratch = Buffer2::new_default(width, height);
    matched_filter(
        &pixels,
        &background,
        3.0,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
    );

    // Peak should be positive and significant
    let peak = result[cy * width + cx];
    assert!(peak > 0.0, "Star should show as positive peak");

    // Background regions should be near zero
    assert!(result[0].abs() < 0.01, "Background should be near zero");
}

#[test]
fn test_matched_filter_boosts_snr() {
    let width = 64;
    let height = 64;
    let background = Buffer2::new(width, height, vec![0.1f32; width * height]);

    // Create star with noise
    let mut pixels = vec![0.1f32; width * height];
    let cx = 32;
    let cy = 32;

    // Add Gaussian-like star
    let sigma = 2.0;
    for dy in -6..=6 {
        for dx in -6..=6 {
            let r2 = (dx * dx + dy * dy) as f32;
            let value = 0.3 * (-r2 / (2.0 * sigma * sigma)).exp();
            let x = (cx as i32 + dx) as usize;
            let y = (cy as i32 + dy) as usize;
            pixels[y * width + x] += value;
        }
    }

    // Add some noise
    for (i, p) in pixels.iter_mut().enumerate() {
        *p += ((i * 17) % 100) as f32 * 0.001 - 0.05;
    }

    let fwhm = sigma * FWHM_TO_SIGMA;
    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    let mut scratch = Buffer2::new_default(width, height);
    matched_filter(
        &pixels,
        &background,
        fwhm,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
    );

    // Peak should be at star location
    let mut max_val = f32::MIN;
    let mut max_x = 0;
    let mut max_y = 0;
    for y in 0..height {
        for x in 0..width {
            let v = result[y * width + x];
            if v > max_val {
                max_val = v;
                max_x = x;
                max_y = y;
            }
        }
    }

    assert_eq!(max_x, cx, "Peak X should be at star center");
    assert_eq!(max_y, cy, "Peak Y should be at star center");
}

#[test]
fn test_matched_filter_clips_negative() {
    let width = 16;
    let height = 16;
    let background = Buffer2::new(width, height, vec![0.5f32; width * height]);
    let pixels = Buffer2::new(width, height, vec![0.3f32; width * height]); // Below background

    let mut result = Buffer2::new_default(width, height);
    let mut scratch = Buffer2::new_default(width, height);
    matched_filter(
        &pixels,
        &background,
        2.0,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
    );

    // Should be clipped to zero before convolution
    for &v in result.iter() {
        assert!(v >= 0.0, "Negative values should be clipped");
    }
}

// ============================================================================
// Performance-related tests
// ============================================================================

#[test]
fn test_separable_vs_direct_equivalence() {
    // For small images, compare separable to direct implementation
    let width = 16;
    let height = 16;
    let mut pixels = vec![0.0f32; width * height];

    // Random-ish pattern
    for (i, p) in pixels.iter_mut().enumerate() {
        *p = ((i * 7 + 3) % 100) as f32 / 100.0;
    }

    let sigma = 1.5;
    let pixels = Buffer2::new(width, height, pixels);
    let mut result_sep = Buffer2::new_default(width, height);
    let mut result_direct = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, sigma, &mut result_sep);
    gaussian_convolve_2d_direct(&pixels, sigma, &mut result_direct);

    for (i, (&a, &b)) in result_sep.iter().zip(result_direct.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-5,
            "Separable and direct should match at {}: {} vs {}",
            i,
            a,
            b
        );
    }
}

#[test]
fn test_large_image_convolution() {
    // Just verify it completes without error
    let width = 512;
    let height = 512;
    let pixels = Buffer2::new(width, height, vec![0.5f32; width * height]);

    let mut result = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 3.0, &mut result);

    assert_eq!(result.len(), width * height);
    assert!(result[0].is_finite());
}

// ============================================================================
// Elliptical convolution tests
// ============================================================================

#[test]
fn test_elliptical_kernel_normalization() {
    for sigma in [1.0, 2.0, 3.0] {
        for axis_ratio in [0.3, 0.5, 0.7, 1.0] {
            for angle in [0.0, 0.5, 1.0, 1.57] {
                let (kernel, _ksize) =
                    super::elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle);
                let sum: f32 = kernel.iter().sum();
                assert!(
                    (sum - 1.0).abs() < 1e-5,
                    "Elliptical kernel should sum to 1.0, got {} for sigma={}, axis_ratio={}, angle={}",
                    sum,
                    sigma,
                    axis_ratio,
                    angle
                );
            }
        }
    }
}

#[test]
fn test_elliptical_kernel_symmetry_at_zero_angle() {
    let (kernel, ksize) = super::elliptical_gaussian_kernel_2d(2.0, 0.5, 0.0);

    // At angle=0, kernel should be symmetric about both axes
    let center = ksize / 2;
    for dy in 0..=center {
        for dx in 0..=center {
            let v1 = kernel[(center + dy) * ksize + (center + dx)];
            let v2 = kernel[(center - dy) * ksize + (center + dx)];
            let v3 = kernel[(center + dy) * ksize + (center - dx)];
            let v4 = kernel[(center - dy) * ksize + (center - dx)];

            assert!(
                (v1 - v2).abs() < 1e-6 && (v1 - v3).abs() < 1e-6 && (v1 - v4).abs() < 1e-6,
                "Kernel should be 4-fold symmetric at angle=0"
            );
        }
    }
}

#[test]
fn test_elliptical_kernel_elongation() {
    // With axis_ratio < 1, kernel should be elongated along major axis
    let (kernel, ksize) = super::elliptical_gaussian_kernel_2d(2.0, 0.3, 0.0);
    let center = ksize / 2;

    // At angle=0, major axis is horizontal (x), minor axis is vertical (y)
    // Check that horizontal extent > vertical extent at same distance from center
    let dist = 2;
    let horizontal_val = kernel[center * ksize + (center + dist)];
    let vertical_val = kernel[(center + dist) * ksize + center];

    assert!(
        horizontal_val > vertical_val,
        "Horizontal extent should be larger than vertical for axis_ratio < 1 at angle=0"
    );
}

#[test]
fn test_elliptical_convolve_uniform_image() {
    let width = 32;
    let height = 32;
    let pixels = Buffer2::new(width, height, vec![0.5f32; width * height]);

    let mut result = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.5, &mut result);

    for v in result.iter() {
        assert!(
            (v - 0.5).abs() < 1e-4,
            "Uniform image should stay uniform after elliptical convolution"
        );
    }
}

#[test]
fn test_elliptical_convolve_preserves_flux() {
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.0f32; width * height];
    pixels[32 * width + 32] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.3, &mut result);

    let input_sum: f32 = pixels.iter().sum();
    let output_sum: f32 = result.iter().sum();

    assert!(
        (output_sum - input_sum).abs() < 0.01,
        "Elliptical convolution should preserve flux: input={}, output={}",
        input_sum,
        output_sum
    );
}

#[test]
fn test_elliptical_convolve_spreads_point_source() {
    let width = 32;
    let height = 32;
    let mut pixels = vec![0.0f32; width * height];
    let cx = 16;
    let cy = 16;
    pixels[cy * width + cx] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.0, &mut result);

    // Peak should be reduced
    let peak = result[cy * width + cx];
    assert!(peak < 1.0, "Peak should be reduced after convolution");
    assert!(peak > 0.01, "Peak should still be significant");

    // Neighbors should have non-zero values
    assert!(result[cy * width + cx + 1] > 0.0);
    assert!(result[(cy + 1) * width + cx] > 0.0);
}

#[test]
fn test_elliptical_convolve_axis_ratio_1_matches_circular() {
    let width = 32;
    let height = 32;
    let mut pixels = vec![0.0f32; width * height];
    for (i, p) in pixels.iter_mut().enumerate() {
        *p = ((i * 7 + 3) % 100) as f32 / 100.0;
    }

    let sigma = 2.0;
    let pixels = Buffer2::new(width, height, pixels);
    let mut result_circular = Buffer2::new_default(width, height);
    let mut result_elliptical = Buffer2::new_default(width, height);

    gaussian_convolve(&pixels, sigma, &mut result_circular);
    elliptical_gaussian_convolve(&pixels, sigma, 1.0, 0.0, &mut result_elliptical);

    for (i, (&a, &b)) in result_circular
        .iter()
        .zip(result_elliptical.iter())
        .enumerate()
    {
        assert!(
            (a - b).abs() < 1e-4,
            "axis_ratio=1.0 should match circular convolution at {}: {} vs {}",
            i,
            a,
            b
        );
    }
}

#[test]
fn test_elliptical_convolve_rotation_invariance() {
    // A point source convolved with elliptical kernel at different angles
    // should produce different orientations but same total flux
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.0f32; width * height];
    pixels[32 * width + 32] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result_0 = Buffer2::new_default(width, height);
    let mut result_90 = Buffer2::new_default(width, height);

    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.0, &mut result_0);
    elliptical_gaussian_convolve(
        &pixels,
        2.0,
        0.5,
        std::f32::consts::FRAC_PI_2,
        &mut result_90,
    );

    let sum_0: f32 = result_0.iter().sum();
    let sum_90: f32 = result_90.iter().sum();

    assert!(
        (sum_0 - sum_90).abs() < 1e-4,
        "Total flux should be same at different angles: {} vs {}",
        sum_0,
        sum_90
    );

    // The patterns should be rotated 90 degrees
    // At angle=0, horizontal spread > vertical
    // At angle=90, vertical spread > horizontal
    let h_spread_0 = result_0[32 * width + 34]; // +2 in x
    let v_spread_0 = result_0[34 * width + 32]; // +2 in y

    let h_spread_90 = result_90[32 * width + 34];
    let v_spread_90 = result_90[34 * width + 32];

    assert!(
        h_spread_0 > v_spread_0,
        "At angle=0, horizontal spread should be larger"
    );
    assert!(
        v_spread_90 > h_spread_90,
        "At angle=90, vertical spread should be larger"
    );
}

#[test]
fn test_elliptical_convolve_various_axis_ratios() {
    let width = 32;
    let height = 32;
    let mut pixels = vec![0.0f32; width * height];
    pixels[16 * width + 16] = 1.0;
    let pixels = Buffer2::new(width, height, pixels);

    for axis_ratio in [0.2, 0.4, 0.6, 0.8, 1.0] {
        let mut result = Buffer2::new_default(width, height);
        elliptical_gaussian_convolve(&pixels, 2.0, axis_ratio, 0.0, &mut result);

        // Check finite values
        for v in result.iter() {
            assert!(
                v.is_finite(),
                "All values should be finite for axis_ratio={}",
                axis_ratio
            );
        }

        // Check flux preservation
        let sum: f32 = result.iter().sum();
        assert!(
            (sum - 1.0).abs() < 0.01,
            "Flux should be preserved for axis_ratio={}: {}",
            axis_ratio,
            sum
        );
    }
}

// ============================================================================
// Numerical precision tests
// ============================================================================

#[test]
fn test_gaussian_kernel_known_values() {
    // For sigma=1.0, the 1D Gaussian at x=0 is 1/(sqrt(2*pi)*sigma) ≈ 0.3989
    // After normalization, center should be the largest
    let kernel = gaussian_kernel_1d(1.0);
    let center = kernel.len() / 2;

    // The kernel is normalized, so we check relative values
    // At x=1, G(1)/G(0) = exp(-0.5) ≈ 0.6065
    let ratio = kernel[center + 1] / kernel[center];
    let expected_ratio = (-0.5f32).exp();

    assert!(
        (ratio - expected_ratio).abs() < 1e-5,
        "Gaussian ratio at x=1 should be exp(-0.5): {} vs {}",
        ratio,
        expected_ratio
    );
}

#[test]
fn test_convolution_linearity() {
    // Convolution should be linear: conv(a + b) = conv(a) + conv(b)
    let width = 32;
    let height = 32;
    let sigma = 2.0;

    let mut pixels_a = vec![0.0f32; width * height];
    let mut pixels_b = vec![0.0f32; width * height];
    pixels_a[16 * width + 12] = 1.0;
    pixels_b[16 * width + 20] = 1.0;

    let mut pixels_sum = vec![0.0f32; width * height];
    for i in 0..pixels_sum.len() {
        pixels_sum[i] = pixels_a[i] + pixels_b[i];
    }

    let pixels_a = Buffer2::new(width, height, pixels_a);
    let pixels_b = Buffer2::new(width, height, pixels_b);
    let pixels_sum = Buffer2::new(width, height, pixels_sum);

    let mut result_a = Buffer2::new_default(width, height);
    let mut result_b = Buffer2::new_default(width, height);
    let mut result_sum = Buffer2::new_default(width, height);

    gaussian_convolve(&pixels_a, sigma, &mut result_a);
    gaussian_convolve(&pixels_b, sigma, &mut result_b);
    gaussian_convolve(&pixels_sum, sigma, &mut result_sum);

    for i in 0..width * height {
        let combined = result_a[i] + result_b[i];
        assert!(
            (combined - result_sum[i]).abs() < 1e-5,
            "Convolution should be linear at {}: {} vs {}",
            i,
            combined,
            result_sum[i]
        );
    }
}

#[test]
fn test_convolution_scaling() {
    // conv(k * f) = k * conv(f)
    let width = 32;
    let height = 32;
    let sigma = 2.0;
    let scale = 3.5;

    let mut pixels = vec![0.0f32; width * height];
    pixels[16 * width + 16] = 1.0;

    let mut pixels_scaled = vec![0.0f32; width * height];
    for i in 0..pixels.len() {
        pixels_scaled[i] = pixels[i] * scale;
    }

    let pixels = Buffer2::new(width, height, pixels);
    let pixels_scaled = Buffer2::new(width, height, pixels_scaled);

    let mut result = Buffer2::new_default(width, height);
    let mut result_scaled = Buffer2::new_default(width, height);

    gaussian_convolve(&pixels, sigma, &mut result);
    gaussian_convolve(&pixels_scaled, sigma, &mut result_scaled);

    for i in 0..width * height {
        assert!(
            (result[i] * scale - result_scaled[i]).abs() < 1e-5,
            "Convolution should scale linearly at {}: {} vs {}",
            i,
            result[i] * scale,
            result_scaled[i]
        );
    }
}
