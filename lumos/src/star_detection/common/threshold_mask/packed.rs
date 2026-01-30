//! Packed bit threshold mask creation with SIMD.
//!
//! Creates threshold masks directly into BitBuffer2's packed u64 storage.
//! This is more efficient than writing individual bools.

use crate::common::BitBuffer2;
use rayon::prelude::*;

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

/// Scalar implementation for packed threshold mask.
///
/// Processes 64 pixels at a time, packing results into u64 words.
#[inline]
fn process_words_scalar<const INCLUDE_BACKGROUND: bool>(
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
            let base = if INCLUDE_BACKGROUND { bg[px_idx] } else { 0.0 };
            let threshold = base + sigma_threshold * noise[px_idx].max(1e-6);

            if px > threshold {
                bits |= 1u64 << bit;
            }
        }

        *word = bits;
    }
}

/// SSE4.1 implementation for packed threshold mask.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn process_words_sse<const INCLUDE_BACKGROUND: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
    use std::arch::x86_64::*;

    let sigma_vec = _mm_set1_ps(sigma_threshold);
    let min_noise_vec = _mm_set1_ps(1e-6);
    let zero_vec = _mm_setzero_ps();

    let pixels_ptr = pixels.as_ptr();
    let bg_ptr = bg.as_ptr();
    let noise_ptr = noise.as_ptr();

    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_offset + word_idx * 64;

        // Check if we have a full 64 pixels
        if base_pixel + 64 <= total_pixels {
            // Full word - process 64 pixels with SIMD
            let mut bits = 0u64;

            // Process 16 groups of 4 floats (SSE processes 4 at a time)
            for group in 0..16 {
                let px_idx = base_pixel + group * 4;

                let px_vec = _mm_loadu_ps(pixels_ptr.add(px_idx));
                let bg_vec = if INCLUDE_BACKGROUND {
                    _mm_loadu_ps(bg_ptr.add(px_idx))
                } else {
                    zero_vec
                };
                let noise_vec = _mm_loadu_ps(noise_ptr.add(px_idx));

                let effective_noise = _mm_max_ps(noise_vec, min_noise_vec);
                let threshold_vec = _mm_add_ps(bg_vec, _mm_mul_ps(sigma_vec, effective_noise));
                let cmp = _mm_cmpgt_ps(px_vec, threshold_vec);
                let mask = _mm_movemask_ps(cmp) as u64;

                bits |= mask << (group * 4);
            }

            *word = bits;
        } else {
            // Partial word - use scalar for remaining pixels
            let mut bits = 0u64;
            for bit in 0..64 {
                let px_idx = base_pixel + bit;
                if px_idx >= total_pixels {
                    break;
                }

                let px = pixels[px_idx];
                let base = if INCLUDE_BACKGROUND { bg[px_idx] } else { 0.0 };
                let threshold = base + sigma_threshold * noise[px_idx].max(1e-6);

                if px > threshold {
                    bits |= 1u64 << bit;
                }
            }
            *word = bits;
        }
    }
}

/// NEON implementation for packed threshold mask.
#[cfg(target_arch = "aarch64")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn process_words_neon<const INCLUDE_BACKGROUND: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    total_pixels: usize,
) {
    use std::arch::aarch64::*;

    let sigma_vec = vdupq_n_f32(sigma_threshold);
    let min_noise_vec = vdupq_n_f32(1e-6);
    let zero_vec = vdupq_n_f32(0.0);

    let pixels_ptr = pixels.as_ptr();
    let bg_ptr = bg.as_ptr();
    let noise_ptr = noise.as_ptr();

    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_offset + word_idx * 64;

        if base_pixel + 64 <= total_pixels {
            let mut bits = 0u64;

            // Process 16 groups of 4 floats (NEON processes 4 at a time)
            for group in 0..16 {
                let px_idx = base_pixel + group * 4;

                let px_vec = vld1q_f32(pixels_ptr.add(px_idx));
                let bg_vec = if INCLUDE_BACKGROUND {
                    vld1q_f32(bg_ptr.add(px_idx))
                } else {
                    zero_vec
                };
                let noise_vec = vld1q_f32(noise_ptr.add(px_idx));

                let effective_noise = vmaxq_f32(noise_vec, min_noise_vec);
                let threshold_vec = vmlaq_f32(bg_vec, sigma_vec, effective_noise);
                let cmp = vcgtq_f32(px_vec, threshold_vec);

                // Extract mask bits from comparison result
                // NEON comparison produces all-ones or all-zeros per lane
                let mask_u32 = vreinterpretq_u32_f32(vreinterpretq_f32_u32(cmp));
                let mask = ((vgetq_lane_u32(mask_u32, 0) & 1) as u64)
                    | (((vgetq_lane_u32(mask_u32, 1) & 1) as u64) << 1)
                    | (((vgetq_lane_u32(mask_u32, 2) & 1) as u64) << 2)
                    | (((vgetq_lane_u32(mask_u32, 3) & 1) as u64) << 3);

                bits |= mask << (group * 4);
            }

            *word = bits;
        } else {
            // Partial word - scalar fallback
            let mut bits = 0u64;
            for bit in 0..64 {
                let px_idx = base_pixel + bit;
                if px_idx >= total_pixels {
                    break;
                }

                let px = pixels[px_idx];
                let base = if INCLUDE_BACKGROUND { bg[px_idx] } else { 0.0 };
                let threshold = base + sigma_threshold * noise[px_idx].max(1e-6);

                if px > threshold {
                    bits |= 1u64 << bit;
                }
            }
            *word = bits;
        }
    }
}

/// Process words with best available SIMD.
#[inline]
fn process_words<const INCLUDE_BACKGROUND: bool>(
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
            process_words_neon::<INCLUDE_BACKGROUND>(
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
                process_words_sse::<INCLUDE_BACKGROUND>(
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

    // Scalar fallback
    process_words_scalar::<INCLUDE_BACKGROUND>(
        pixels,
        bg,
        noise,
        sigma_threshold,
        words,
        pixel_offset,
        total_pixels,
    );
}

/// Internal dispatch to parallel implementation.
pub fn create_threshold_mask_packed_impl<const INCLUDE_BACKGROUND: bool>(
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

    // Process in parallel chunks of words
    // Each word covers 64 pixels, so we chunk words
    const WORDS_PER_CHUNK: usize = 1024; // 64K pixels per chunk

    words
        .par_chunks_mut(WORDS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk_idx, word_chunk)| {
            let word_offset = chunk_idx * WORDS_PER_CHUNK;
            let pixel_offset = word_offset * 64;

            process_words::<INCLUDE_BACKGROUND>(
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

/// Create binary mask of pixels above threshold into a BitBuffer2.
///
/// Sets bit `i` to 1 where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// Uses SIMD acceleration when available (SSE4.1 on x86_64, NEON on aarch64).
/// Writes directly to packed u64 words for better memory efficiency.
#[allow(dead_code)] // Public API - will be used as Buffer2<bool> is migrated
pub fn create_threshold_mask_packed(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    mask: &mut BitBuffer2,
) {
    create_threshold_mask_packed_impl::<true>(pixels, bg, noise, sigma_threshold, mask);
}

/// Create binary mask from a filtered (background-subtracted) image.
///
/// Sets bit `i` to 1 where `filtered[i] > sigma * noise[i]`.
/// Used for matched-filtered images where background is already subtracted.
#[allow(dead_code)] // Public API for external use
pub fn create_threshold_mask_filtered_packed(
    filtered: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    mask: &mut BitBuffer2,
) {
    // For filtered images, background is already subtracted, so pass zeros
    let zeros = vec![0.0f32; filtered.len()];
    create_threshold_mask_packed_impl::<false>(filtered, &zeros, noise, sigma_threshold, mask);
}

#[cfg(test)]
mod tests {
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
        create_threshold_mask_packed(&pixels, &bg, &noise, sigma, &mut packed_mask);

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
        create_threshold_mask_packed(&pixels, &bg, &noise, 3.0, &mut mask);

        // All should be set
        assert_eq!(mask.count_ones(), size);
    }
}
