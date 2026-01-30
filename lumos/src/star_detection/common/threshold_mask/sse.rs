//! SSE4.1 SIMD implementation for packed threshold mask.

/// SSE4.1 implementation for packed threshold mask.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn process_words_sse<const INCLUDE_BACKGROUND: bool>(
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
