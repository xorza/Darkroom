//! NEON SIMD implementation for packed threshold mask.

use std::arch::aarch64::*;

use crate::stacking::star_detection::threshold_mask::{MIN_NOISE, process_words_scalar};

/// NEON packed threshold kernel. `WITH_BG` selects `bg + σ·noise` vs `σ·noise` (filtered); `bg` is
/// unused and may be empty when `WITH_BG` is false. Uses unfused multiply-then-add (not `vfmaq_f32`)
/// to stay bit-exact with the scalar / AVX2 / SSE backends at the `px == threshold` boundary.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn process_words_neon<const WITH_BG: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    pixel_end: usize,
) {
    let sigma_vec = vdupq_n_f32(sigma_threshold);
    let min_noise_vec = vdupq_n_f32(MIN_NOISE);

    let pixels_ptr = pixels.as_ptr();
    let bg_ptr = bg.as_ptr();
    let noise_ptr = noise.as_ptr();

    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_offset + word_idx * 64;

        if base_pixel + 64 <= pixel_end {
            let mut bits = 0u64;

            for group in 0..16 {
                let px_idx = base_pixel + group * 4;

                let px_vec = vld1q_f32(pixels_ptr.add(px_idx));
                let noise_vec = vld1q_f32(noise_ptr.add(px_idx));
                let effective_noise = vmaxq_f32(noise_vec, min_noise_vec);

                let threshold_vec = if WITH_BG {
                    let bg_vec = vld1q_f32(bg_ptr.add(px_idx));
                    vaddq_f32(bg_vec, vmulq_f32(sigma_vec, effective_noise))
                } else {
                    vmulq_f32(sigma_vec, effective_noise)
                };

                // `vcgtq_f32` already yields a uint32x4_t lane mask.
                let cmp = vcgtq_f32(px_vec, threshold_vec);
                let mask = ((vgetq_lane_u32(cmp, 0) & 1) as u64)
                    | (((vgetq_lane_u32(cmp, 1) & 1) as u64) << 1)
                    | (((vgetq_lane_u32(cmp, 2) & 1) as u64) << 2)
                    | (((vgetq_lane_u32(cmp, 3) & 1) as u64) << 3);

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
