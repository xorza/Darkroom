//! SIMD-optimized threshold mask creation.
//!
//! Creates binary masks marking pixels above a sigma threshold relative to
//! background and noise estimates. Used by both background estimation
//! (to mask bright objects) and detection (to find star candidates).

pub mod scalar;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

use crate::common::Buffer2;
use crate::star_detection::background::BackgroundMap;

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

/// Create binary mask of pixels above threshold.
///
/// Sets `mask[i] = true` where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// Uses SIMD acceleration when available (SSE4.1 on x86_64, NEON on aarch64).
#[cfg(target_arch = "x86_64")]
pub fn create_threshold_mask(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    if cpu_features::has_sse4_1() {
        // SAFETY: We've checked that SSE4.1 is available.
        unsafe {
            sse::create_threshold_mask_sse_impl::<true>(pixels, background, sigma_threshold, mask);
        }
    } else {
        scalar::create_threshold_mask_impl::<true>(pixels, background, sigma_threshold, mask);
    }
}

/// Create binary mask of pixels above threshold (aarch64).
#[cfg(target_arch = "aarch64")]
pub fn create_threshold_mask(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    // SAFETY: NEON is always available on aarch64.
    unsafe {
        neon::create_threshold_mask_neon_impl::<true>(pixels, background, sigma_threshold, mask);
    }
}

/// Create binary mask of pixels above threshold (scalar fallback).
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub fn create_threshold_mask(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    scalar::create_threshold_mask_impl::<true>(pixels, background, sigma_threshold, mask);
}

/// Create binary mask from a filtered (background-subtracted) image.
///
/// Sets `mask[i] = true` where `filtered[i] > sigma * noise[i]`.
/// Used for matched-filtered images where background is already subtracted.
#[cfg(target_arch = "x86_64")]
pub fn create_threshold_mask_filtered(
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    if cpu_features::has_sse4_1() {
        // SAFETY: We've checked that SSE4.1 is available.
        unsafe {
            sse::create_threshold_mask_sse_impl::<false>(
                filtered,
                background,
                sigma_threshold,
                mask,
            );
        }
    } else {
        scalar::create_threshold_mask_impl::<false>(filtered, background, sigma_threshold, mask);
    }
}

/// Create binary mask from a filtered image (aarch64).
#[cfg(target_arch = "aarch64")]
pub fn create_threshold_mask_filtered(
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    // SAFETY: NEON is always available on aarch64.
    unsafe {
        neon::create_threshold_mask_neon_impl::<false>(filtered, background, sigma_threshold, mask);
    }
}

/// Create binary mask from a filtered image (scalar fallback).
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub fn create_threshold_mask_filtered(
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    scalar::create_threshold_mask_impl::<false>(filtered, background, sigma_threshold, mask);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_data(len: usize) -> (Buffer2<f32>, BackgroundMap, Buffer2<bool>) {
        let width = len;
        let height = 1;
        let pixels = Buffer2::new_filled(width, height, 100.0);
        let background = BackgroundMap {
            background: Buffer2::new_filled(width, height, 50.0),
            noise: Buffer2::new_filled(width, height, 10.0),
        };
        let mask = Buffer2::new_filled(width, height, false);
        (pixels, background, mask)
    }

    #[test]
    fn test_threshold_mask_above() {
        let (pixels, background, mut mask) = create_test_data(10);
        // threshold = 50 + 3 * 10 = 80, pixels = 100 > 80
        create_threshold_mask(&pixels, &background, 3.0, &mut mask);
        assert!(mask.iter().all(|&v| v));
    }

    #[test]
    fn test_threshold_mask_below() {
        let (mut pixels, background, mut mask) = create_test_data(10);
        pixels.fill(60.0);
        // threshold = 50 + 3 * 10 = 80, pixels = 60 < 80
        create_threshold_mask(&pixels, &background, 3.0, &mut mask);
        assert!(mask.iter().all(|&v| !v));
    }

    #[test]
    fn test_threshold_mask_filtered() {
        let width = 10;
        let height = 1;
        let filtered = Buffer2::new_filled(width, height, 50.0);
        let background = BackgroundMap {
            background: Buffer2::new_filled(width, height, 0.0), // not used
            noise: Buffer2::new_filled(width, height, 10.0),
        };
        let mut mask = Buffer2::new_filled(width, height, false);
        // threshold = 3 * 10 = 30, filtered = 50 > 30
        create_threshold_mask_filtered(&filtered, &background, 3.0, &mut mask);
        assert!(mask.iter().all(|&v| v));
    }

    #[test]
    fn test_various_lengths() {
        // Test edge cases for SIMD remainder handling
        for len in [1, 3, 4, 5, 15, 16, 17, 31, 32, 33, 100] {
            let (pixels, background, mut mask) = create_test_data(len);
            create_threshold_mask(&pixels, &background, 3.0, &mut mask);
            assert!(mask.iter().all(|&v| v), "failed for len={}", len);
        }
    }
}
