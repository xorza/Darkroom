//! NEON SIMD implementation for packed threshold mask.

/// NEON implementation for packed threshold mask.
#[cfg(target_arch = "aarch64")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn process_words_neon<const INCLUDE_BACKGROUND: bool>(
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
