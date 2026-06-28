//! SSE4.1 SIMD implementation for packed threshold mask.

use std::arch::x86_64::*;

use crate::stacking::star_detection::threshold_mask::{MIN_NOISE, process_words_scalar};

/// SSE4.1 packed threshold kernel. `WITH_BG` selects `bg + σ·noise` vs `σ·noise` (filtered); `bg`
/// is unused and may be empty when `WITH_BG` is false.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn process_words_sse<const WITH_BG: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    pixel_end: usize,
) {
    let sigma_vec = _mm_set1_ps(sigma_threshold);
    let min_noise_vec = _mm_set1_ps(MIN_NOISE);

    let pixels_ptr = pixels.as_ptr();
    let bg_ptr = bg.as_ptr();
    let noise_ptr = noise.as_ptr();

    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_offset + word_idx * 64;

        if base_pixel + 64 <= pixel_end {
            let mut bits = 0u64;

            for group in 0..16 {
                let px_idx = base_pixel + group * 4;

                let px_vec = _mm_loadu_ps(pixels_ptr.add(px_idx));
                let noise_vec = _mm_loadu_ps(noise_ptr.add(px_idx));
                let effective_noise = _mm_max_ps(noise_vec, min_noise_vec);

                let threshold_vec = if WITH_BG {
                    let bg_vec = _mm_loadu_ps(bg_ptr.add(px_idx));
                    _mm_add_ps(bg_vec, _mm_mul_ps(sigma_vec, effective_noise))
                } else {
                    _mm_mul_ps(sigma_vec, effective_noise)
                };

                let cmp = _mm_cmpgt_ps(px_vec, threshold_vec);
                let mask = _mm_movemask_ps(cmp) as u64;

                bits |= mask << (group * 4);
            }

            *word = bits;
        } else {
            // Partial trailing word — defer to the shared scalar kernel for this one word.
            process_words_scalar::<WITH_BG>(
                pixels,
                bg,
                noise,
                sigma_threshold,
                std::slice::from_mut(word),
                base_pixel,
                pixel_end,
            );
        }
    }
}
