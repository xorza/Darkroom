//! SIMD-accelerated star detection utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE4.1 on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

use super::super::background::BackgroundMap;

/// Create threshold mask using SIMD acceleration.
///
/// Computes: mask[i] = pixels[i] > (background[i] + sigma * noise[i])
///
/// This function dispatches to the best available SIMD implementation at runtime.
#[inline]
pub fn create_threshold_mask_simd(
    pixels: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
) -> Vec<bool> {
    let len = pixels.len();
    let mut output = vec![false; len];

    #[cfg(target_arch = "x86_64")]
    {
        if len >= 8 && cpu_features::has_avx2() {
            unsafe {
                sse::create_threshold_mask_avx2(
                    pixels,
                    &background.background,
                    &background.noise,
                    sigma_threshold,
                    &mut output,
                );
            }
            return output;
        }
        if len >= 4 && cpu_features::has_sse4_1() {
            unsafe {
                sse::create_threshold_mask_sse41(
                    pixels,
                    &background.background,
                    &background.noise,
                    sigma_threshold,
                    &mut output,
                );
            }
            return output;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if len >= 4 {
            unsafe {
                neon::create_threshold_mask_neon(
                    pixels,
                    &background.background,
                    &background.noise,
                    sigma_threshold,
                    &mut output,
                );
            }
            return output;
        }
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    {
        for i in 0..len {
            let threshold =
                background.background[i] + sigma_threshold * background.noise[i].max(1e-6);
            output[i] = pixels[i] > threshold;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_background_map(
        width: usize,
        height: usize,
        bg_val: f32,
        noise_val: f32,
    ) -> BackgroundMap {
        let len = width * height;
        BackgroundMap {
            background: vec![bg_val; len],
            noise: vec![noise_val; len],
            width,
            height,
        }
    }

    fn create_threshold_mask_scalar(
        pixels: &[f32],
        background: &BackgroundMap,
        sigma: f32,
    ) -> Vec<bool> {
        pixels
            .iter()
            .zip(background.background.iter())
            .zip(background.noise.iter())
            .map(|((&px, &bg), &n)| {
                let threshold = bg + sigma * n.max(1e-6);
                px > threshold
            })
            .collect()
    }

    #[test]
    fn test_create_threshold_mask_simd_basic() {
        let len = 100;
        let pixels: Vec<f32> = (0..len).map(|i| (i as f32) * 0.01).collect();
        let background = BackgroundMap {
            background: (0..len).map(|i| (i as f32) * 0.005).collect(),
            noise: vec![0.05; len],
            width: 10,
            height: 10,
        };
        let sigma = 3.0;

        let result = create_threshold_mask_simd(&pixels, &background, sigma);

        // Verify against scalar computation
        for i in 0..len {
            let threshold = background.background[i] + sigma * background.noise[i].max(1e-6);
            let expected = pixels[i] > threshold;
            assert_eq!(
                result[i], expected,
                "Mismatch at index {}: got {}, expected {}",
                i, result[i], expected
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_detects_stars() {
        let len = 64;
        let mut pixels = vec![0.1f32; len];
        pixels[10] = 0.5; // Bright star
        pixels[30] = 0.3; // Moderate star
        pixels[50] = 0.12; // Just above threshold

        let background = create_background_map(8, 8, 0.1, 0.01);
        let sigma = 5.0; // Threshold = 0.1 + 5.0 * 0.01 = 0.15

        let result = create_threshold_mask_simd(&pixels, &background, sigma);

        assert!(result[10], "Bright star should be detected");
        assert!(result[30], "Moderate star should be detected");
        assert!(!result[50], "Below-threshold pixel should not be detected");
        assert!(!result[0], "Background pixel should not be detected");
    }

    #[test]
    fn test_create_threshold_mask_simd_small_array() {
        // Test with array smaller than SIMD width
        let pixels = vec![0.5, 0.1, 0.3];
        let background = BackgroundMap {
            background: vec![0.1, 0.1, 0.1],
            noise: vec![0.05, 0.05, 0.05],
            width: 3,
            height: 1,
        };
        let sigma = 3.0; // Threshold = 0.1 + 0.15 = 0.25

        let result = create_threshold_mask_simd(&pixels, &background, sigma);

        assert!(result[0], "0.5 > 0.25");
        assert!(!result[1], "0.1 < 0.25");
        assert!(result[2], "0.3 > 0.25");
    }

    #[test]
    fn test_create_threshold_mask_simd_empty() {
        let pixels: Vec<f32> = vec![];
        let background = BackgroundMap {
            background: vec![],
            noise: vec![],
            width: 0,
            height: 0,
        };

        let result = create_threshold_mask_simd(&pixels, &background, 3.0);
        assert!(result.is_empty());
    }

    // ========== Comprehensive SIMD vs Scalar tests ==========

    #[test]
    fn test_create_threshold_mask_simd_matches_scalar_various_sizes() {
        // Test various array sizes including those smaller/larger than SIMD widths
        for size in [
            1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100, 256,
        ] {
            let pixels: Vec<f32> = (0..size).map(|i| (i as f32 * 0.1).sin() + 0.5).collect();
            let background = BackgroundMap {
                background: (0..size)
                    .map(|i| (i as f32 * 0.05).cos() * 0.2 + 0.3)
                    .collect(),
                noise: (0..size).map(|i| 0.02 + (i as f32 * 0.001)).collect(),
                width: size,
                height: 1,
            };
            let sigma = 3.0;

            let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
            let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

            for i in 0..size {
                assert_eq!(
                    result_simd[i], result_scalar[i],
                    "Size {}, index {}: SIMD={} vs Scalar={}",
                    size, i, result_simd[i], result_scalar[i]
                );
            }
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_all_above_threshold() {
        let len = 100;
        let pixels = vec![1.0f32; len]; // All high
        let background = create_background_map(10, 10, 0.1, 0.01);
        let sigma = 5.0; // Threshold = 0.15

        let result = create_threshold_mask_simd(&pixels, &background, sigma);

        for (i, &val) in result.iter().enumerate() {
            assert!(val, "All pixels should be above threshold, failed at {}", i);
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_all_below_threshold() {
        let len = 100;
        let pixels = vec![0.0f32; len]; // All zero
        let background = create_background_map(10, 10, 0.1, 0.01);
        let sigma = 5.0; // Threshold = 0.15

        let result = create_threshold_mask_simd(&pixels, &background, sigma);

        for (i, &val) in result.iter().enumerate() {
            assert!(
                !val,
                "All pixels should be below threshold, failed at {}",
                i
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_at_threshold_boundary() {
        // Test pixels exactly at or near threshold boundary
        let len = 64;
        let background = create_background_map(8, 8, 0.1, 0.01);
        let sigma = 5.0;
        // Threshold = 0.1 + 5.0 * 0.01 = 0.15

        // Create pixels at exact boundary and slightly above/below
        let mut pixels = vec![0.15f32; len]; // Exactly at threshold
        pixels[10] = 0.14999; // Just below
        pixels[20] = 0.15001; // Just above
        pixels[30] = 0.1499; // Below
        pixels[40] = 0.1501; // Above

        let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
        let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

        for i in 0..len {
            assert_eq!(
                result_simd[i], result_scalar[i],
                "Boundary test at {}: SIMD={} vs Scalar={}",
                i, result_simd[i], result_scalar[i]
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_varying_noise() {
        let len = 100;
        let pixels: Vec<f32> = (0..len).map(|i| 0.3 + (i as f32 * 0.003)).collect();
        let background = BackgroundMap {
            background: vec![0.1; len],
            noise: (0..len).map(|i| 0.01 + (i as f32 * 0.001)).collect(), // Increasing noise
            width: 10,
            height: 10,
        };
        let sigma = 3.0;

        let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
        let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

        for i in 0..len {
            assert_eq!(
                result_simd[i], result_scalar[i],
                "Varying noise at {}: SIMD={} vs Scalar={}",
                i, result_simd[i], result_scalar[i]
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_varying_background() {
        let len = 100;
        let pixels = vec![0.5f32; len]; // Constant pixels
        let background = BackgroundMap {
            background: (0..len).map(|i| 0.1 + (i as f32 * 0.005)).collect(), // Increasing bg
            noise: vec![0.02; len],
            width: 10,
            height: 10,
        };
        let sigma = 3.0;

        let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
        let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

        for i in 0..len {
            assert_eq!(
                result_simd[i], result_scalar[i],
                "Varying background at {}: SIMD={} vs Scalar={}",
                i, result_simd[i], result_scalar[i]
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_different_sigma_values() {
        let len = 64;
        let pixels: Vec<f32> = (0..len).map(|i| i as f32 * 0.02).collect();
        let background = create_background_map(8, 8, 0.1, 0.05);

        for sigma in [0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0] {
            let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
            let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

            for i in 0..len {
                assert_eq!(
                    result_simd[i], result_scalar[i],
                    "Sigma {}, index {}: SIMD={} vs Scalar={}",
                    sigma, i, result_simd[i], result_scalar[i]
                );
            }
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_very_small_noise() {
        // Test with noise near the 1e-6 clamp threshold
        let len = 64;
        let pixels: Vec<f32> = (0..len).map(|i| 0.2 + (i as f32 * 0.001)).collect();
        let background = BackgroundMap {
            background: vec![0.1; len],
            noise: vec![1e-7; len], // Below clamp threshold
            width: 8,
            height: 8,
        };
        let sigma = 3.0;

        let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
        let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

        for i in 0..len {
            assert_eq!(
                result_simd[i], result_scalar[i],
                "Very small noise at {}: SIMD={} vs Scalar={}",
                i, result_simd[i], result_scalar[i]
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_zero_noise() {
        let len = 64;
        let pixels: Vec<f32> = (0..len).map(|i| 0.2 + (i as f32 * 0.001)).collect();
        let background = BackgroundMap {
            background: vec![0.1; len],
            noise: vec![0.0; len], // Zero noise (should clamp to 1e-6)
            width: 8,
            height: 8,
        };
        let sigma = 3.0;

        let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
        let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

        for i in 0..len {
            assert_eq!(
                result_simd[i], result_scalar[i],
                "Zero noise at {}: SIMD={} vs Scalar={}",
                i, result_simd[i], result_scalar[i]
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_random_pattern() {
        let len = 256;
        let pixels: Vec<f32> = (0..len)
            .map(|i| (i as f32 * 0.7).sin() * 0.3 + 0.5)
            .collect();
        let background = BackgroundMap {
            background: (0..len)
                .map(|i| (i as f32 * 0.3).cos() * 0.1 + 0.2)
                .collect(),
            noise: (0..len)
                .map(|i| 0.02 + (i as f32 * 0.5).sin().abs() * 0.03)
                .collect(),
            width: 16,
            height: 16,
        };
        let sigma = 3.0;

        let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
        let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

        for i in 0..len {
            assert_eq!(
                result_simd[i], result_scalar[i],
                "Random pattern at {}: SIMD={} vs Scalar={}",
                i, result_simd[i], result_scalar[i]
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_large_array() {
        let len = 4096;
        let pixels: Vec<f32> = (0..len)
            .map(|i| (i as f32 * 0.01).sin() * 0.4 + 0.5)
            .collect();
        let background = BackgroundMap {
            background: (0..len).map(|i| 0.1 + (i % 100) as f32 * 0.001).collect(),
            noise: (0..len).map(|i| 0.01 + (i % 50) as f32 * 0.0005).collect(),
            width: 64,
            height: 64,
        };
        let sigma = 3.0;

        let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
        let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

        for i in 0..len {
            assert_eq!(
                result_simd[i], result_scalar[i],
                "Large array at {}: SIMD={} vs Scalar={}",
                i, result_simd[i], result_scalar[i]
            );
        }
    }

    #[test]
    fn test_create_threshold_mask_simd_alternating_pattern() {
        // Test with alternating above/below threshold pattern
        let len = 64;
        let pixels: Vec<f32> = (0..len)
            .map(|i| if i % 2 == 0 { 0.5 } else { 0.0 })
            .collect();
        let background = create_background_map(8, 8, 0.1, 0.05);
        let sigma = 3.0; // Threshold = 0.1 + 0.15 = 0.25

        let result_simd = create_threshold_mask_simd(&pixels, &background, sigma);
        let result_scalar = create_threshold_mask_scalar(&pixels, &background, sigma);

        for i in 0..len {
            assert_eq!(
                result_simd[i], result_scalar[i],
                "Alternating at {}: SIMD={} vs Scalar={}",
                i, result_simd[i], result_scalar[i]
            );
            // Also verify expected pattern
            let expected = i % 2 == 0; // 0.5 > 0.25, 0.0 < 0.25
            assert_eq!(result_simd[i], expected, "Expected pattern at {}", i);
        }
    }
}
