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
}
