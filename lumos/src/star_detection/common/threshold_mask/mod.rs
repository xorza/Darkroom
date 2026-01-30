//! SIMD-optimized threshold mask creation.
//!
//! Creates binary masks marking pixels above a sigma threshold relative to
//! background and noise estimates. Used by both background estimation
//! (to mask bright objects) and detection (to find star candidates).
//!
//! Uses bit-packed storage (`BitBuffer2`) for memory efficiency - each pixel
//! uses 1 bit instead of 1 byte, reducing memory usage by 8x.

#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse;

#[cfg(test)]
mod bench;

#[cfg(test)]
mod tests;

use crate::common::BitBuffer2;
use crate::common::parallel::ParChunksMutAutoWithOffset;
use rayon::iter::ParallelIterator;

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

/// Scalar implementation for packed threshold mask with background.
#[cfg_attr(not(test), inline)]
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
#[inline]
fn process_words_filtered_scalar(
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

/// Process words with best available SIMD (with background).
#[cfg_attr(not(test), inline)]
pub(super) fn process_words(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
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
        return;
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

/// Process words with best available SIMD (without background, for filtered images).
#[inline]
fn process_words_filtered(
    pixels: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
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
        return;
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

/// Create binary mask of pixels above threshold into a BitBuffer2.
///
/// Sets bit `i` to 1 where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// Uses SIMD acceleration when available (SSE4.1 on x86_64, NEON on aarch64).
/// Writes directly to packed u64 words for better memory efficiency.
pub fn create_threshold_mask(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    mask: &mut BitBuffer2,
) {
    let total_pixels = pixels.len();
    debug_assert_eq!(total_pixels, mask.len());
    debug_assert_eq!(total_pixels, bg.len());
    debug_assert_eq!(total_pixels, noise.len());

    let words = mask.words_mut();

    words
        .par_chunks_mut_auto()
        .for_each(|(word_offset, word_chunk)| {
            let pixel_offset = word_offset * 64;

            process_words(
                pixels,
                bg,
                noise,
                sigma_threshold,
                word_chunk,
                pixel_offset,
                total_pixels,
            );
        });
}

/// Create binary mask from a filtered (background-subtracted) image.
///
/// Sets bit `i` to 1 where `filtered[i] > sigma * noise[i]`.
/// Used for matched-filtered images where background is already subtracted.
pub fn create_threshold_mask_filtered(
    filtered: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    mask: &mut BitBuffer2,
) {
    let total_pixels = filtered.len();
    debug_assert_eq!(total_pixels, mask.len());
    debug_assert_eq!(total_pixels, noise.len());

    let words = mask.words_mut();

    words
        .par_chunks_mut_auto()
        .for_each(|(word_offset, word_chunk)| {
            let pixel_offset = word_offset * 64;

            process_words_filtered(
                filtered,
                noise,
                sigma_threshold,
                word_chunk,
                pixel_offset,
                total_pixels,
            );
        });
}

#[cfg(test)]
mod mask_tests {
    use super::*;

    /// Reference scalar implementation for testing
    fn scalar_threshold(pixels: &[f32], bg: &[f32], noise: &[f32], sigma: f32) -> Vec<bool> {
        pixels
            .iter()
            .zip(bg.iter())
            .zip(noise.iter())
            .map(|((&px, &b), &n)| {
                let threshold = b + sigma * n.max(1e-6);
                px > threshold
            })
            .collect()
    }

    #[test]
    fn test_packed_matches_scalar() {
        let width = 100;
        let height = 100;
        let size = width * height;

        let mut pixels = vec![0.0f32; size];
        let mut bg = vec![1.0f32; size];
        let mut noise = vec![0.1f32; size];

        // Create some test pattern
        for i in 0..size {
            pixels[i] = ((i * 17) % 100) as f32 / 50.0;
            bg[i] = 1.0 + ((i * 7) % 10) as f32 / 100.0;
            noise[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0;
        }

        let sigma = 3.0;

        // Compute with scalar reference
        let scalar_mask = scalar_threshold(&pixels, &bg, &noise, sigma);

        // Compute with packed BitBuffer2
        let mut packed_mask = BitBuffer2::new_filled(width, height, false);
        create_threshold_mask(&pixels, &bg, &noise, sigma, &mut packed_mask);

        // Compare results
        for (i, &scalar_val) in scalar_mask.iter().enumerate() {
            assert_eq!(
                scalar_val,
                packed_mask.get(i),
                "Mismatch at index {}: scalar={}, packed={}",
                i,
                scalar_val,
                packed_mask.get(i)
            );
        }
    }

    #[test]
    fn test_packed_non_aligned_size() {
        // Test with size that doesn't align to 64 bits
        let width = 100;
        let height = 73; // 7300 pixels, not divisible by 64
        let size = width * height;

        let pixels = vec![2.0f32; size]; // All above threshold
        let bg = vec![0.0f32; size];
        let noise = vec![0.1f32; size];

        let mut mask = BitBuffer2::new_filled(width, height, false);
        create_threshold_mask(&pixels, &bg, &noise, 3.0, &mut mask);

        // All should be set
        assert_eq!(mask.count_ones(), size);
    }
}
