//! ARM NEON-accelerated threshold mask creation.

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

use crate::common::Buffer2;
use crate::star_detection::background::BackgroundMap;

/// Number of SIMD vectors to process per unrolled iteration.
const UNROLL_FACTOR: usize = 4;
/// Floats per NEON vector.
const NEON_WIDTH: usize = 4;
/// Floats processed per unrolled iteration.
const UNROLL_WIDTH: usize = UNROLL_FACTOR * NEON_WIDTH;

/// NEON-accelerated threshold mask creation.
///
/// When `INCLUDE_BACKGROUND` is true:
///   Sets `mask[i] = true` where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// When `INCLUDE_BACKGROUND` is false:
///   Sets `mask[i] = true` where `pixels[i] > sigma * noise[i]`.
///
/// # Safety
/// Requires NEON support (always available on aarch64).
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn create_threshold_mask_neon_impl<const INCLUDE_BACKGROUND: bool>(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    let len = pixels.len();
    debug_assert_eq!(len, mask.len());

    let mask_ptr = mask.pixels_mut().as_mut_ptr();

    let sigma_vec = vdupq_n_f32(sigma_threshold);
    let min_noise_vec = vdupq_n_f32(1e-6);
    let zero_vec = vdupq_n_f32(0.0);

    let pixels_ptr = pixels.as_ptr();
    let bg_ptr = background.background.as_ptr();
    let noise_ptr = background.noise.as_ptr();

    // Process 16 floats (4 NEON vectors) per unrolled iteration
    let unrolled_end = len - (len % UNROLL_WIDTH);
    let mut i = 0;

    while i < unrolled_end {
        // Load 4 vectors worth of data
        let px0 = vld1q_f32(pixels_ptr.add(i));
        let px1 = vld1q_f32(pixels_ptr.add(i + 4));
        let px2 = vld1q_f32(pixels_ptr.add(i + 8));
        let px3 = vld1q_f32(pixels_ptr.add(i + 12));

        let (bg0, bg1, bg2, bg3) = if INCLUDE_BACKGROUND {
            (
                vld1q_f32(bg_ptr.add(i)),
                vld1q_f32(bg_ptr.add(i + 4)),
                vld1q_f32(bg_ptr.add(i + 8)),
                vld1q_f32(bg_ptr.add(i + 12)),
            )
        } else {
            (zero_vec, zero_vec, zero_vec, zero_vec)
        };

        let noise0 = vld1q_f32(noise_ptr.add(i));
        let noise1 = vld1q_f32(noise_ptr.add(i + 4));
        let noise2 = vld1q_f32(noise_ptr.add(i + 8));
        let noise3 = vld1q_f32(noise_ptr.add(i + 12));

        // threshold = bg + sigma * noise.max(1e-6)
        let eff_noise0 = vmaxq_f32(noise0, min_noise_vec);
        let eff_noise1 = vmaxq_f32(noise1, min_noise_vec);
        let eff_noise2 = vmaxq_f32(noise2, min_noise_vec);
        let eff_noise3 = vmaxq_f32(noise3, min_noise_vec);

        let thresh0 = vaddq_f32(bg0, vmulq_f32(sigma_vec, eff_noise0));
        let thresh1 = vaddq_f32(bg1, vmulq_f32(sigma_vec, eff_noise1));
        let thresh2 = vaddq_f32(bg2, vmulq_f32(sigma_vec, eff_noise2));
        let thresh3 = vaddq_f32(bg3, vmulq_f32(sigma_vec, eff_noise3));

        // px > threshold - vcgtq_f32 returns 0xFFFFFFFF for true, 0 for false
        let cmp0 = vcgtq_f32(px0, thresh0);
        let cmp1 = vcgtq_f32(px1, thresh1);
        let cmp2 = vcgtq_f32(px2, thresh2);
        let cmp3 = vcgtq_f32(px3, thresh3);

        // Extract comparison results and write to mask
        *mask_ptr.add(i) = vgetq_lane_u32(cmp0, 0) != 0;
        *mask_ptr.add(i + 1) = vgetq_lane_u32(cmp0, 1) != 0;
        *mask_ptr.add(i + 2) = vgetq_lane_u32(cmp0, 2) != 0;
        *mask_ptr.add(i + 3) = vgetq_lane_u32(cmp0, 3) != 0;

        *mask_ptr.add(i + 4) = vgetq_lane_u32(cmp1, 0) != 0;
        *mask_ptr.add(i + 5) = vgetq_lane_u32(cmp1, 1) != 0;
        *mask_ptr.add(i + 6) = vgetq_lane_u32(cmp1, 2) != 0;
        *mask_ptr.add(i + 7) = vgetq_lane_u32(cmp1, 3) != 0;

        *mask_ptr.add(i + 8) = vgetq_lane_u32(cmp2, 0) != 0;
        *mask_ptr.add(i + 9) = vgetq_lane_u32(cmp2, 1) != 0;
        *mask_ptr.add(i + 10) = vgetq_lane_u32(cmp2, 2) != 0;
        *mask_ptr.add(i + 11) = vgetq_lane_u32(cmp2, 3) != 0;

        *mask_ptr.add(i + 12) = vgetq_lane_u32(cmp3, 0) != 0;
        *mask_ptr.add(i + 13) = vgetq_lane_u32(cmp3, 1) != 0;
        *mask_ptr.add(i + 14) = vgetq_lane_u32(cmp3, 2) != 0;
        *mask_ptr.add(i + 15) = vgetq_lane_u32(cmp3, 3) != 0;

        i += UNROLL_WIDTH;
    }

    // Process remaining full NEON vectors (0-3 vectors)
    while i + NEON_WIDTH <= len {
        let px_vec = vld1q_f32(pixels_ptr.add(i));
        let bg_vec = if INCLUDE_BACKGROUND {
            vld1q_f32(bg_ptr.add(i))
        } else {
            zero_vec
        };
        let noise_vec = vld1q_f32(noise_ptr.add(i));

        let effective_noise = vmaxq_f32(noise_vec, min_noise_vec);
        let threshold_vec = vaddq_f32(bg_vec, vmulq_f32(sigma_vec, effective_noise));
        let cmp = vcgtq_f32(px_vec, threshold_vec);

        *mask_ptr.add(i) = vgetq_lane_u32(cmp, 0) != 0;
        *mask_ptr.add(i + 1) = vgetq_lane_u32(cmp, 1) != 0;
        *mask_ptr.add(i + 2) = vgetq_lane_u32(cmp, 2) != 0;
        *mask_ptr.add(i + 3) = vgetq_lane_u32(cmp, 3) != 0;

        i += NEON_WIDTH;
    }

    // Handle remaining elements (0-3 elements)
    let pixels_slice = pixels.pixels();
    while i < len {
        let base = if INCLUDE_BACKGROUND {
            background.background[i]
        } else {
            0.0
        };
        let threshold = base + sigma_threshold * background.noise[i].max(1e-6);
        *mask_ptr.add(i) = pixels_slice[i] > threshold;
        i += 1;
    }
}
