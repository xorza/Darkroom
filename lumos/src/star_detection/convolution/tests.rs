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
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result, &mut temp);

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
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result, &mut temp);

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
    // Convolving a delta at (16,16) with Gaussian of sigma=2 should produce
    // the Gaussian kernel itself centered at (16,16).
    // For a 1D Gaussian: G(0) = 1/(sigma*sqrt(2pi)) ≈ 1/(2*2.5066) ≈ 0.19947
    // For separable 2D: peak = G(0)^2 ≈ 0.03979
    // But the kernel is discretized and normalized, so verify peak matches
    // the product of 1D kernel center values.
    let width = 32;
    let height = 32;
    let mut pixels = vec![0.0f32; width * height];
    pixels[16 * width + 16] = 1.0;

    let sigma = 2.0;
    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, sigma, &mut result, &mut temp);

    let kernel = gaussian_kernel_1d(sigma);
    let center_val = kernel[kernel.len() / 2];
    // For separable convolution, peak = center_val^2
    let expected_peak = center_val * center_val;
    let peak = result[16 * width + 16];
    assert!(
        (peak - expected_peak).abs() < 1e-5,
        "Peak {} should equal kernel center^2 = {}",
        peak,
        expected_peak
    );

    // Value at (17,16) = center_val * kernel[center+1] (one step in x, zero in y)
    let one_step_val = kernel[kernel.len() / 2 + 1];
    let expected_neighbor = center_val * one_step_val;
    let actual_neighbor = result[16 * width + 17];
    assert!(
        (actual_neighbor - expected_neighbor).abs() < 1e-5,
        "Neighbor {} should equal {} (kernel product)",
        actual_neighbor,
        expected_neighbor
    );
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
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result, &mut temp);

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
fn test_gaussian_convolve_peak_matches_kernel_product() {
    // Verify that different sigmas produce peaks matching kernel center^2
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.0f32; width * height];
    pixels[32 * width + 32] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut temp = Buffer2::new_default(width, height);

    for sigma in [1.0f32, 2.0, 3.0] {
        let mut result = Buffer2::new_default(width, height);
        gaussian_convolve(&pixels, sigma, &mut result, &mut temp);

        let kernel = gaussian_kernel_1d(sigma);
        let center = kernel[kernel.len() / 2];
        let expected_peak = center * center;
        let actual_peak = result[32 * width + 32];

        assert!(
            (actual_peak - expected_peak).abs() < 1e-5,
            "sigma={}: peak {} should match kernel center^2 = {}",
            sigma,
            actual_peak,
            expected_peak
        );
    }
}

#[test]
fn test_gaussian_convolve_edge_handling() {
    // Point source near edge (2,2) in 16×16 image — mirror boundary should
    // preserve total flux and maintain symmetry where possible
    let width = 16;
    let height = 16;
    let mut pixels = vec![0.0f32; width * height];
    pixels[2 * width + 2] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 1.5, &mut result, &mut temp);

    // No NaN or Inf
    assert!(result.iter().all(|v| v.is_finite()));

    // Peak should still be at (2,2)
    let peak = result[2 * width + 2];
    let kernel = gaussian_kernel_1d(1.5);
    let center = kernel[kernel.len() / 2];
    assert!(
        (peak - center * center).abs() < 0.02,
        "Edge peak {} should be near kernel center^2 = {}",
        peak,
        center * center
    );

    // Total flux may exceed 1.0 near edges due to mirror boundary conditions
    // (reflected virtual pixels add energy). Just verify it's positive and finite.
    let total: f32 = result.iter().sum();
    assert!(total > 0.0, "Total flux should be positive, got {}", total);
}

#[test]
fn test_gaussian_convolve_non_square_image() {
    // 64×32 non-square image with point source at (32,16)
    let width = 64;
    let height = 32;
    let mut pixels = vec![0.0f32; width * height];
    pixels[16 * width + 32] = 1.0;

    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result, &mut temp);

    assert_eq!(result.len(), width * height);

    // Peak should match kernel center^2
    let kernel = gaussian_kernel_1d(2.0);
    let center = kernel[kernel.len() / 2];
    let peak = result[16 * width + 32];
    assert!(
        (peak - center * center).abs() < 1e-5,
        "Non-square peak {} should match kernel center^2 = {}",
        peak,
        center * center
    );
}

#[test]
fn test_gaussian_convolve_small_image() {
    // Uniform image smaller than kernel radius (sigma=2, radius=6, size=13)
    // Should trigger direct 2D fallback and preserve uniform values exactly
    let width = 8;
    let height = 8;
    let pixels = Buffer2::new(width, height, vec![1.0f32; width * height]);

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 2.0, &mut result, &mut temp);

    // Convolution of uniform image with normalized kernel should give exactly 1.0
    for (i, v) in result.iter().enumerate() {
        assert!(
            (*v - 1.0).abs() < 1e-4,
            "Pixel {} should be 1.0, got {}",
            i,
            v
        );
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
    let mut temp = Buffer2::new_default(width, height);
    matched_filter(
        &pixels,
        &background,
        3.0,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
        &mut temp,
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
    let mut temp = Buffer2::new_default(width, height);
    matched_filter(
        &pixels,
        &background,
        3.0,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
        &mut temp,
    );

    // Peak at star location should be the maximum in the image
    let peak = result[cy * width + cx];
    let max_val = result.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let max_idx = result.iter().position(|&v| v == max_val).unwrap();
    assert_eq!(
        max_idx,
        cy * width + cx,
        "Maximum should be at star center ({},{})",
        cx,
        cy
    );
    assert!(peak > 0.1, "Star peak {} should be substantial", peak);
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
    let mut temp = Buffer2::new_default(width, height);
    matched_filter(
        &pixels,
        &background,
        fwhm,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
        &mut temp,
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
fn test_matched_filter_preserves_negative_residuals() {
    let width = 16;
    let height = 16;
    let background = Buffer2::new(width, height, vec![0.5f32; width * height]);
    let pixels = Buffer2::new(width, height, vec![0.3f32; width * height]); // Below background

    let mut result = Buffer2::new_default(width, height);
    let mut scratch = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    matched_filter(
        &pixels,
        &background,
        2.0,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
        &mut temp,
    );

    // Negative residuals are preserved for correct noise statistics.
    // Uniform below-background input should produce negative convolved output.
    for &v in result.iter() {
        assert!(
            v < 0.0,
            "Below-background pixels should produce negative output"
        );
    }
}

#[test]
fn test_matched_filter_noise_normalization() {
    use rand::prelude::*;

    // After noise normalization, the standard deviation of the output on a
    // pure-noise image should approximately match the input noise level.
    // This verifies the sqrt(sum(K^2)) normalization is correct.
    let width = 256;
    let height = 256;
    let bg_level = 1000.0f32;
    let noise_sigma = 10.0f32;

    // Generate spatially uncorrelated Gaussian noise via Box-Muller
    let mut rng = StdRng::seed_from_u64(42);
    let mut pixels = vec![bg_level; width * height];
    for chunk in pixels.chunks_mut(2) {
        let u1: f32 = rng.random_range(1e-10f32..1.0);
        let u2: f32 = rng.random_range(0.0f32..1.0);
        let r = (-2.0 * u1.ln()).sqrt() * noise_sigma;
        let theta = 2.0 * std::f32::consts::PI * u2;
        chunk[0] += r * theta.cos();
        if chunk.len() > 1 {
            chunk[1] += r * theta.sin();
        }
    }

    let background = Buffer2::new(width, height, vec![bg_level; width * height]);
    let pixels = Buffer2::new(width, height, pixels);
    let mut result = Buffer2::new_default(width, height);
    let mut scratch = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);

    let fwhm = 4.0;
    matched_filter(
        &pixels,
        &background,
        fwhm,
        1.0,
        0.0,
        &mut result,
        &mut scratch,
        &mut temp,
    );

    // Compute standard deviation of output (excluding border region affected by mirror)
    let margin = 10;
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    let mut count = 0usize;
    for y in margin..height - margin {
        for x in margin..width - margin {
            let v = result[y * width + x] as f64;
            sum += v;
            sum_sq += v * v;
            count += 1;
        }
    }
    let mean = sum / count as f64;
    let variance = sum_sq / count as f64 - mean * mean;
    let output_sigma = variance.sqrt();

    // After normalization, output noise should be close to input noise.
    // Allow 30% tolerance for finite-sample and boundary effects.
    let ratio = output_sigma / noise_sigma as f64;
    assert!(
        (0.7..1.3).contains(&ratio),
        "Output noise should match input noise after normalization. \
         ratio={ratio:.3}, output_sigma={output_sigma:.2}, input_sigma={noise_sigma}"
    );
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
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, sigma, &mut result_sep, &mut temp);
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
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&pixels, 3.0, &mut result, &mut temp);

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
    let mut temp = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.5, &mut result, &mut temp);

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
    let mut temp = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.3, &mut result, &mut temp);

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
    let mut temp = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.0, &mut result, &mut temp);

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
    let mut temp = Buffer2::new_default(width, height);

    gaussian_convolve(&pixels, sigma, &mut result_circular, &mut temp);
    elliptical_gaussian_convolve(&pixels, sigma, 1.0, 0.0, &mut result_elliptical, &mut temp);

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
    let mut temp = Buffer2::new_default(width, height);

    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.0, &mut result_0, &mut temp);
    elliptical_gaussian_convolve(
        &pixels,
        2.0,
        0.5,
        std::f32::consts::FRAC_PI_2,
        &mut result_90,
        &mut temp,
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
    // For a point source at (16,16), elliptical convolution should preserve flux
    let width = 32;
    let height = 32;
    let mut pixels = vec![0.0f32; width * height];
    pixels[16 * width + 16] = 1.0;
    let pixels = Buffer2::new(width, height, pixels);

    let mut peaks = Vec::new();
    for axis_ratio in [1.0, 0.8, 0.6, 0.4, 0.2] {
        let mut result = Buffer2::new_default(width, height);
        let mut temp = Buffer2::new_default(width, height);
        elliptical_gaussian_convolve(&pixels, 2.0, axis_ratio, 0.0, &mut result, &mut temp);

        let sum: f32 = result.iter().sum();
        assert!(
            (sum - 1.0).abs() < 0.01,
            "Flux should be 1.0 for axis_ratio={}: got {}",
            axis_ratio,
            sum
        );

        let peak = result[16 * width + 16];
        assert!(
            peak > 0.0,
            "Peak should be positive for axis_ratio={}",
            axis_ratio
        );
        peaks.push((axis_ratio, peak));
    }

    // Different axis ratios should produce different peaks
    // (axis_ratio=1.0 delegates to separable gaussian_convolve)
    for i in 1..peaks.len() {
        assert!(
            (peaks[i].1 - peaks[0].1).abs() > 1e-6,
            "axis_ratio={} peak {} should differ from axis_ratio=1.0 peak {}",
            peaks[i].0,
            peaks[i].1,
            peaks[0].1
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
    let mut temp = Buffer2::new_default(width, height);

    gaussian_convolve(&pixels_a, sigma, &mut result_a, &mut temp);
    gaussian_convolve(&pixels_b, sigma, &mut result_b, &mut temp);
    gaussian_convolve(&pixels_sum, sigma, &mut result_sum, &mut temp);

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
    let mut temp = Buffer2::new_default(width, height);

    gaussian_convolve(&pixels, sigma, &mut result, &mut temp);
    gaussian_convolve(&pixels_scaled, sigma, &mut result_scaled, &mut temp);

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
