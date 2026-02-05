//! SIMD-optimized threshold mask creation.
//!
//! Creates binary masks marking pixels above a sigma threshold relative to
//! background and noise estimates. Used by both background estimation
//! (to mask bright objects) and detection (to find star candidates).
//!
//! Uses bit-packed storage (`BitBuffer2`) for memory efficiency - each pixel
//! uses 1 bit instead of 1 byte, reducing memory usage by 8x.

use rayon::prelude::*;

#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse;

#[cfg(test)]
mod bench;

#[cfg(test)]
mod tests;

use crate::common::{BitBuffer2, Buffer2};
use rayon::prelude::*;

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

/// Scalar implementation for packed threshold mask with background.
#[cfg_attr(not(test), inline)]
#[cfg(any(test, target_arch = "x86_64", not(target_arch = "aarch64")))]
pub(super) fn process_words_scalar(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_offset + word_idx * 64;
        let mut bits = 0u64;

        for bit in 0..64 {
            let px_idx = base_pixel + bit;
            if px_idx >= total_pixels {
                break;
            }

            let px = pixels[px_idx];
            let threshold = bg[px_idx] + sigma_threshold * noise[px_idx].max(1e-6);

            if px > threshold {
                bits |= 1u64 << bit;
            }
        }

        *word = bits;
    }
}

/// Scalar implementation for packed threshold mask without background (filtered).
#[cfg_attr(not(test), inline)]
#[cfg(any(test, target_arch = "x86_64", not(target_arch = "aarch64")))]
pub(super) fn process_words_filtered_scalar(
    pixels: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_offset + word_idx * 64;
        let mut bits = 0u64;

        for bit in 0..64 {
            let px_idx = base_pixel + bit;
            if px_idx >= total_pixels {
                break;
            }

            let px = pixels[px_idx];
            let threshold = sigma_threshold * noise[px_idx].max(1e-6);

            if px > threshold {
                bits |= 1u64 << bit;
            }
        }

        *word = bits;
    }
}

/// Scalar implementation for adaptive threshold mask with per-pixel sigma.
#[cfg_attr(not(test), inline)]
#[cfg(any(test, target_arch = "x86_64", not(target_arch = "aarch64")))]
pub(super) fn process_words_adaptive_scalar(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    adaptive_sigma: &[f32],
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_offset + word_idx * 64;
        let mut bits = 0u64;

        for bit in 0..64 {
            let px_idx = base_pixel + bit;
            if px_idx >= total_pixels {
                break;
            }

            let px = pixels[px_idx];
            let sigma = adaptive_sigma[px_idx];
            let threshold = bg[px_idx] + sigma * noise[px_idx].max(1e-6);

            if px > threshold {
                bits |= 1u64 << bit;
            }
        }

        *word = bits;
    }
}

/// Process words with best available SIMD (with background).
#[cfg_attr(not(test), inline)]
pub(super) fn process_words(
    pixels: &Buffer2<f32>,
    bg: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
    let pixels = pixels.pixels();
    let bg = bg.pixels();
    let noise = noise.pixels();

    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            neon::process_words_neon(
                pixels,
                bg,
                noise,
                sigma_threshold,
                words,
                pixel_offset,
                total_pixels,
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_sse4_1() {
            // SAFETY: We've checked that SSE4.1 is available.
            unsafe {
                sse::process_words_sse(
                    pixels,
                    bg,
                    noise,
                    sigma_threshold,
                    words,
                    pixel_offset,
                    total_pixels,
                );
            }
            return;
        }

        process_words_scalar(
            pixels,
            bg,
            noise,
            sigma_threshold,
            words,
            pixel_offset,
            total_pixels,
        );
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        process_words_scalar(
            pixels,
            bg,
            noise,
            sigma_threshold,
            words,
            pixel_offset,
            total_pixels,
        );
    }
}

/// Process words with best available SIMD (without background, for filtered images).
#[inline]
pub(super) fn process_words_filtered(
    pixels: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
    let pixels = pixels.pixels();
    let noise = noise.pixels();

    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            neon::process_words_filtered_neon(
                pixels,
                noise,
                sigma_threshold,
                words,
                pixel_offset,
                total_pixels,
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_sse4_1() {
            // SAFETY: We've checked that SSE4.1 is available.
            unsafe {
                sse::process_words_filtered_sse(
                    pixels,
                    noise,
                    sigma_threshold,
                    words,
                    pixel_offset,
                    total_pixels,
                );
            }
            return;
        }

        process_words_filtered_scalar(
            pixels,
            noise,
            sigma_threshold,
            words,
            pixel_offset,
            total_pixels,
        );
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        process_words_filtered_scalar(
            pixels,
            noise,
            sigma_threshold,
            words,
            pixel_offset,
            total_pixels,
        );
    }
}

/// Process words with adaptive per-pixel sigma threshold.
#[inline]
pub(super) fn process_words_adaptive(
    pixels: &Buffer2<f32>,
    bg: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    adaptive_sigma: &Buffer2<f32>,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
    let pixels = pixels.pixels();
    let bg = bg.pixels();
    let noise = noise.pixels();
    let adaptive_sigma = adaptive_sigma.pixels();

    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            neon::process_words_adaptive_neon(
                pixels,
                bg,
                noise,
                adaptive_sigma,
                words,
                pixel_offset,
                total_pixels,
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_sse4_1() {
            // SAFETY: We've checked that SSE4.1 is available.
            unsafe {
                sse::process_words_adaptive_sse(
                    pixels,
                    bg,
                    noise,
                    adaptive_sigma,
                    words,
                    pixel_offset,
                    total_pixels,
                );
            }
            return;
        }

        process_words_adaptive_scalar(
            pixels,
            bg,
            noise,
            adaptive_sigma,
            words,
            pixel_offset,
            total_pixels,
        );
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        process_words_adaptive_scalar(
            pixels,
            bg,
            noise,
            adaptive_sigma,
            words,
            pixel_offset,
            total_pixels,
        );
    }
}

/// Create binary mask of pixels above threshold into a BitBuffer2.
///
/// Sets bit `i` to 1 where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// Uses SIMD acceleration when available (SSE4.1 on x86_64, NEON on aarch64).
/// Writes directly to packed u64 words for better memory efficiency.
///
/// Note: All input buffers must have the same dimensions as the mask.
/// The output mask has row-aligned storage (stride may differ from width).
pub fn create_threshold_mask(
    pixels: &Buffer2<f32>,
    bg: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    sigma_threshold: f32,
    mask: &mut BitBuffer2,
) {
    let width = mask.width();
    let height = mask.height();
    debug_assert_eq!(width, pixels.width());
    debug_assert_eq!(height, pixels.height());
    debug_assert_eq!(width, bg.width());
    debug_assert_eq!(height, bg.height());
    debug_assert_eq!(width, noise.width());
    debug_assert_eq!(height, noise.height());

    let words_per_row = mask.words_per_row();

    mask.words_mut()
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, row_words)| {
            let row_pixel_start = y * width;
            process_words(
                pixels,
                bg,
                noise,
                sigma_threshold,
                row_words,
                row_pixel_start,
                row_pixel_start + width,
            );
        });
}

/// Create binary mask using per-pixel adaptive sigma thresholds.
///
/// Sets bit `i` to 1 where `pixels[i] > background[i] + adaptive_sigma[i] * noise[i]`.
///
/// This is used for adaptive thresholding where the sigma threshold varies
/// across the image based on local contrast (higher in nebulous regions,
/// lower in uniform sky regions).
///
/// Note: All input buffers must have the same dimensions as the mask.
/// The output mask has row-aligned storage (stride may differ from width).
pub fn create_adaptive_threshold_mask(
    pixels: &Buffer2<f32>,
    bg: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    adaptive_sigma: &Buffer2<f32>,
    mask: &mut BitBuffer2,
) {
    let width = mask.width();
    let height = mask.height();
    debug_assert_eq!(width, pixels.width());
    debug_assert_eq!(height, pixels.height());
    debug_assert_eq!(width, bg.width());
    debug_assert_eq!(height, bg.height());
    debug_assert_eq!(width, noise.width());
    debug_assert_eq!(height, noise.height());
    debug_assert_eq!(width, adaptive_sigma.width());
    debug_assert_eq!(height, adaptive_sigma.height());

    let words_per_row = mask.words_per_row();

    mask.words_mut()
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, row_words)| {
            let row_pixel_start = y * width;
            process_words_adaptive(
                pixels,
                bg,
                noise,
                adaptive_sigma,
                row_words,
                row_pixel_start,
                row_pixel_start + width,
            );
        });
}

/// Create binary mask from a filtered (background-subtracted) image.
///
/// Sets bit `i` to 1 where `filtered[i] > sigma * noise[i]`.
/// Used for matched-filtered images where background is already subtracted.
///
/// Note: All input buffers must have the same dimensions as the mask.
/// The output mask has row-aligned storage (stride may differ from width).
pub fn create_threshold_mask_filtered(
    filtered: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    sigma_threshold: f32,
    mask: &mut BitBuffer2,
) {
    let width = mask.width();
    let height = mask.height();
    debug_assert_eq!(width, filtered.width());
    debug_assert_eq!(height, filtered.height());
    debug_assert_eq!(width, noise.width());
    debug_assert_eq!(height, noise.height());

    let words_per_row = mask.words_per_row();

    mask.words_mut()
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, row_words)| {
            let row_pixel_start = y * width;
            process_words_filtered(
                filtered,
                noise,
                sigma_threshold,
                row_words,
                row_pixel_start,
                row_pixel_start + width,
            );
        });
}
