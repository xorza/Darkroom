//! Tests for Gaussian convolution.

use super::*;
use crate::star_detection::Buffer2;
use crate::star_detection::constants::FWHM_TO_SIGMA;

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
// FWHM to sigma conversion tests
// ============================================================================

#[test]
fn test_fwhm_to_sigma() {
    // FWHM = 2.355 * sigma
    let fwhm = 4.71; // ~2 * sigma
    let sigma = fwhm_to_sigma(fwhm);
    assert!((sigma - 2.0).abs() < 0.01);
}

#[test]
fn test_fwhm_to_sigma_roundtrip() {
    use crate::star_detection::constants::FWHM_TO_SIGMA;
    let sigma = 3.0;
    let fwhm = sigma * FWHM_TO_SIGMA;
    let sigma_back = fwhm_to_sigma(fwhm);
    assert!((sigma_back - sigma).abs() < 1e-6);
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

    let result = gaussian_convolve(&pixels, 2.0);

    for v in result {
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
    let result = gaussian_convolve(&pixels, 2.0);

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
    let result = gaussian_convolve(&pixels, sigma);

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
    let result = gaussian_convolve(&pixels, 2.0);

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
    let result_small = gaussian_convolve(&pixels, 1.0);
    let result_large = gaussian_convolve(&pixels, 3.0);

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
    let result = gaussian_convolve(&pixels, 1.5);

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
    let result = gaussian_convolve(&pixels, 2.0);

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

    let result = gaussian_convolve(&pixels, 2.0);

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

    let result = matched_filter(&pixels, &background, 3.0);

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
    let result = matched_filter(&pixels, &background, 3.0);

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
    let result = matched_filter(&pixels, &background, fwhm);

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

    let result = matched_filter(&pixels, &background, 2.0);

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
    let result_sep = gaussian_convolve(&pixels, sigma);
    let result_direct = gaussian_convolve_2d_direct(&pixels, sigma);

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

    let result = gaussian_convolve(&pixels, 3.0);

    assert_eq!(result.len(), width * height);
    assert!(result[0].is_finite());
}
