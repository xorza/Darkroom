//! ARM NEON SIMD implementation of centroid refinement.

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

use crate::math::{FWHM_TO_SIGMA, fast_exp};
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::centroid::is_valid_stamp_position;

/// ARM NEON SIMD implementation of centroid refinement.
///
/// Processes 4 pixels at a time using 128-bit NEON registers.
/// NEON is always available on aarch64.
///
/// # Safety
/// Caller must ensure aarch64 target (NEON is mandatory on aarch64).
#[cfg(target_arch = "aarch64")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn refine_centroid_neon(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    expected_fwhm: f32,
) -> Option<(f32, f32)> {
    if !is_valid_stamp_position(cx, cy, width, height, stamp_radius) {
        return None;
    }

    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

    // Adaptive sigma based on expected FWHM
    let sigma = (expected_fwhm / FWHM_TO_SIGMA * 0.8).clamp(1.0, stamp_radius as f32 * 0.5);
    let two_sigma_sq = 2.0 * sigma * sigma;
    let neg_inv_two_sigma_sq = -1.0 / two_sigma_sq;

    // SIMD constants for fast_exp: exp(x) ≈ reinterpret(A*x + B) as f32
    // A = 2^23 / ln(2), B = 127 * 2^23 - 486411
    let (sum_x, sum_y, sum_w) = unsafe {
        let a_vec = vdupq_n_f32(12102203.0);
        let b_vec = vdupq_n_s32(1065353216 - 486411);
        let neg_inv_two_sigma_sq_vec = vdupq_n_f32(neg_inv_two_sigma_sq);
        let cx_vec = vdupq_n_f32(cx);
        let zero_vec = vdupq_n_f32(0.0);

        let mut sum_x_vec = vdupq_n_f32(0.0);
        let mut sum_y_vec = vdupq_n_f32(0.0);
        let mut sum_w_vec = vdupq_n_f32(0.0);

        let stamp_radius_i32 = stamp_radius as i32;
        let stamp_width = (2 * stamp_radius + 1) as i32;

        for dy in -stamp_radius_i32..=stamp_radius_i32 {
            let y = (icy + dy as isize) as usize;
            let fy = y as f32 - cy;
            let fy_sq = fy * fy;
            let fy_sq_vec = vdupq_n_f32(fy_sq);
            let y_vec = vdupq_n_f32(y as f32);

            let row_start_x = (icx - stamp_radius_i32 as isize) as usize;
            let row_idx = y * width + row_start_x;

            // Process 4 pixels at a time
            let mut dx = 0i32;
            while dx + 4 <= stamp_width {
                let base_x = row_start_x + dx as usize;

                // Load 4 pixel values and backgrounds
                let pix_ptr = pixels.as_ptr().add(row_idx + dx as usize);
                let bg_ptr = background.background.as_ptr().add(row_idx + dx as usize);
                let pix_vec = vld1q_f32(pix_ptr);
                let bg_vec = vld1q_f32(bg_ptr);

                // Background-subtracted value, clamped to >= 0
                let value_vec = vmaxq_f32(vsubq_f32(pix_vec, bg_vec), zero_vec);

                // X coordinates for these 4 pixels
                let x_offsets: float32x4_t = core::mem::transmute([0.0f32, 1.0f32, 2.0f32, 3.0f32]);
                let x_vec = vaddq_f32(vdupq_n_f32(base_x as f32), x_offsets);

                // Distance squared: (x - cx)² + (y - cy)²
                let fx_vec = vsubq_f32(x_vec, cx_vec);
                let fx_sq_vec = vmulq_f32(fx_vec, fx_vec);
                let dist_sq_vec = vaddq_f32(fx_sq_vec, fy_sq_vec);

                // Gaussian weight: exp(-dist_sq / two_sigma_sq)
                // Using Schraudolph's fast exp: reinterpret(A * x + B) as f32
                let exp_arg = vmulq_f32(dist_sq_vec, neg_inv_two_sigma_sq_vec);
                let exp_scaled = vmulq_f32(exp_arg, a_vec);
                let exp_int = vaddq_s32(vcvtq_s32_f32(exp_scaled), b_vec);
                let gauss_weight = vreinterpretq_f32_s32(exp_int);

                // Weight = value * gauss_weight
                let weight_vec = vmulq_f32(value_vec, gauss_weight);

                // Accumulate weighted sums using FMA
                sum_x_vec = vfmaq_f32(sum_x_vec, x_vec, weight_vec);
                sum_y_vec = vfmaq_f32(sum_y_vec, y_vec, weight_vec);
                sum_w_vec = vaddq_f32(sum_w_vec, weight_vec);

                dx += 4;
            }

            // Handle remaining pixels (< 4) with scalar code
            while dx < stamp_width {
                let x = row_start_x + dx as usize;
                let idx = row_idx + dx as usize;

                let value = (pixels[idx] - background.background[idx]).max(0.0);
                let fx = x as f32 - cx;
                let dist_sq = fx * fx + fy_sq;
                let weight = value * fast_exp(-dist_sq / two_sigma_sq);

                // Add to scalar accumulators (will be combined with SIMD results)
                let scalar_x: float32x4_t =
                    core::mem::transmute([x as f32 * weight, 0.0f32, 0.0f32, 0.0f32]);
                let scalar_y: float32x4_t =
                    core::mem::transmute([y as f32 * weight, 0.0f32, 0.0f32, 0.0f32]);
                let scalar_w: float32x4_t = core::mem::transmute([weight, 0.0f32, 0.0f32, 0.0f32]);

                sum_x_vec = vaddq_f32(sum_x_vec, scalar_x);
                sum_y_vec = vaddq_f32(sum_y_vec, scalar_y);
                sum_w_vec = vaddq_f32(sum_w_vec, scalar_w);

                dx += 1;
            }
        }

        // Horizontal sum of SIMD accumulators
        (
            horizontal_sum_neon(sum_x_vec),
            horizontal_sum_neon(sum_y_vec),
            horizontal_sum_neon(sum_w_vec),
        )
    };

    if sum_w < f32::EPSILON {
        return None;
    }

    let new_cx = sum_x / sum_w;
    let new_cy = sum_y / sum_w;

    // Reject if centroid moved too far (likely bad detection)
    let stamp_size = 2 * stamp_radius + 1;
    let max_move = stamp_size as f32 / 4.0;
    if (new_cx - cx).abs() > max_move || (new_cy - cy).abs() > max_move {
        return None;
    }

    Some((new_cx, new_cy))
}

/// Horizontal sum of a NEON f32x4 vector (4 elements).
#[cfg(target_arch = "aarch64")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn horizontal_sum_neon(v: float32x4_t) -> f32 {
    // vaddvq_f32 is the NEON horizontal add instruction
    vaddvq_f32(v)
}
