//! AVX2 SIMD implementation of centroid refinement.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::math::{FWHM_TO_SIGMA, fast_exp};
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::centroid::is_valid_stamp_position;

/// AVX2 SIMD implementation of centroid refinement.
///
/// Processes 8 pixels at a time using 256-bit SIMD registers.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn refine_centroid_avx2(
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
    // All SIMD operations in one unsafe block for Rust 2024 compatibility
    let (sum_x, sum_y, sum_w) = unsafe {
        let a_vec = _mm256_set1_ps(12102203.0);
        let b_vec = _mm256_set1_epi32(1065353216 - 486411);
        let neg_inv_two_sigma_sq_vec = _mm256_set1_ps(neg_inv_two_sigma_sq);
        let cx_vec = _mm256_set1_ps(cx);
        let zero_vec = _mm256_setzero_ps();

        let mut sum_x_vec = _mm256_setzero_ps();
        let mut sum_y_vec = _mm256_setzero_ps();
        let mut sum_w_vec = _mm256_setzero_ps();

        let stamp_radius_i32 = stamp_radius as i32;
        let stamp_width = (2 * stamp_radius + 1) as i32;

        for dy in -stamp_radius_i32..=stamp_radius_i32 {
            let y = (icy + dy as isize) as usize;
            let fy = y as f32 - cy;
            let fy_sq = fy * fy;
            let fy_sq_vec = _mm256_set1_ps(fy_sq);
            let y_vec = _mm256_set1_ps(y as f32);

            let row_start_x = (icx - stamp_radius_i32 as isize) as usize;
            let row_idx = y * width + row_start_x;

            // Process 8 pixels at a time
            let mut dx = 0i32;
            while dx + 8 <= stamp_width {
                let base_x = row_start_x + dx as usize;

                // Load 8 pixel values and backgrounds
                let pix_ptr = pixels.as_ptr().add(row_idx + dx as usize);
                let bg_ptr = background.background.as_ptr().add(row_idx + dx as usize);
                let pix_vec = _mm256_loadu_ps(pix_ptr);
                let bg_vec = _mm256_loadu_ps(bg_ptr);

                // Background-subtracted value, clamped to >= 0
                let value_vec = _mm256_max_ps(_mm256_sub_ps(pix_vec, bg_vec), zero_vec);

                // X coordinates for these 8 pixels
                let x_offsets = _mm256_set_ps(7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.0);
                let x_vec = _mm256_add_ps(_mm256_set1_ps(base_x as f32), x_offsets);

                // Distance squared: (x - cx)² + (y - cy)²
                let fx_vec = _mm256_sub_ps(x_vec, cx_vec);
                let fx_sq_vec = _mm256_mul_ps(fx_vec, fx_vec);
                let dist_sq_vec = _mm256_add_ps(fx_sq_vec, fy_sq_vec);

                // Gaussian weight: exp(-dist_sq / two_sigma_sq)
                // Using Schraudolph's fast exp: reinterpret(A * x + B) as f32
                let exp_arg = _mm256_mul_ps(dist_sq_vec, neg_inv_two_sigma_sq_vec);
                let exp_scaled = _mm256_mul_ps(exp_arg, a_vec);
                let exp_int = _mm256_add_epi32(_mm256_cvtps_epi32(exp_scaled), b_vec);
                let gauss_weight = _mm256_castsi256_ps(exp_int);

                // Weight = value * gauss_weight
                let weight_vec = _mm256_mul_ps(value_vec, gauss_weight);

                // Accumulate weighted sums
                sum_x_vec = _mm256_fmadd_ps(x_vec, weight_vec, sum_x_vec);
                sum_y_vec = _mm256_fmadd_ps(y_vec, weight_vec, sum_y_vec);
                sum_w_vec = _mm256_add_ps(sum_w_vec, weight_vec);

                dx += 8;
            }

            // Handle remaining pixels (< 8) with scalar code
            while dx < stamp_width {
                let x = row_start_x + dx as usize;
                let idx = row_idx + dx as usize;

                let value = (pixels[idx] - background.background[idx]).max(0.0);
                let fx = x as f32 - cx;
                let dist_sq = fx * fx + fy_sq;
                let weight = value * fast_exp(-dist_sq / two_sigma_sq);

                // Add to scalar accumulators (will be combined with SIMD results)
                sum_x_vec = _mm256_add_ps(
                    sum_x_vec,
                    _mm256_set_ps(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, x as f32 * weight),
                );
                sum_y_vec = _mm256_add_ps(
                    sum_y_vec,
                    _mm256_set_ps(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, y as f32 * weight),
                );
                sum_w_vec = _mm256_add_ps(
                    sum_w_vec,
                    _mm256_set_ps(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, weight),
                );

                dx += 1;
            }
        }

        // Horizontal sum of SIMD accumulators
        (
            horizontal_sum_256(sum_x_vec),
            horizontal_sum_256(sum_y_vec),
            horizontal_sum_256(sum_w_vec),
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

/// Horizontal sum of a 256-bit f32 vector (8 elements).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn horizontal_sum_256(v: __m256) -> f32 {
    let high = _mm256_extractf128_ps(v, 1);
    let low = _mm256_castps256_ps128(v);
    let sum128 = _mm_add_ps(low, high);
    let shuf = _mm_movehdup_ps(sum128);
    let sums = _mm_add_ps(sum128, shuf);
    let shuf = _mm_movehl_ps(sums, sums);
    let sums = _mm_add_ss(sums, shuf);
    _mm_cvtss_f32(sums)
}
