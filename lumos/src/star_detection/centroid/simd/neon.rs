//! ARM NEON SIMD implementation of centroid refinement.

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

use glam::Vec2;

use crate::math::{FWHM_TO_SIGMA, fast_exp};
use crate::star_detection::background::BackgroundEstimate;
use crate::star_detection::centroid::is_valid_stamp_position;

/// ARM NEON SIMD implementation of centroid refinement.
///
/// Processes 4 pixels at a time using 128-bit NEON registers.
/// NEON is always available on aarch64.
///
/// # Safety
/// Caller must ensure aarch64 target (NEON is mandatory on aarch64).
#[cfg(target_arch = "aarch64")]
pub unsafe fn refine_centroid_neon(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundEstimate,
    pos: Vec2,
    stamp_radius: usize,
    expected_fwhm: f32,
) -> Option<Vec2> {
    if !is_valid_stamp_position(pos, width, height, stamp_radius) {
        return None;
    }

    let cx = pos.x;
    let cy = pos.y;
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;

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

    let new_pos = Vec2::new(sum_x / sum_w, sum_y / sum_w);

    // Reject if centroid moved too far (likely bad detection)
    let stamp_size = 2 * stamp_radius + 1;
    let max_move = stamp_size as f32 / 4.0;
    if (new_pos - pos).abs().max_element() > max_move {
        return None;
    }

    Some(new_pos)
}

/// Horizontal sum of a NEON f32x4 vector (4 elements).
#[cfg(target_arch = "aarch64")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn horizontal_sum_neon(v: float32x4_t) -> f32 {
    // vaddvq_f32 is the NEON horizontal add instruction
    vaddvq_f32(v)
}

#[cfg(test)]
#[cfg(target_arch = "aarch64")]
mod tests {
    use super::*;
    use crate::common::Buffer2;
    use crate::star_detection::centroid::simd::refine_centroid_scalar;
    use crate::star_detection::config::Config;
    use glam::Vec2;

    fn make_gaussian_star(
        width: usize,
        height: usize,
        pos: Vec2,
        sigma: f32,
        amplitude: f32,
        background: f32,
    ) -> Buffer2<f32> {
        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - pos.x;
                let dy = y as f32 - pos.y;
                let r2 = dx * dx + dy * dy;
                let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                if value > 0.001 {
                    pixels[y * width + x] += value;
                }
            }
        }
        Buffer2::new(width, height, pixels)
    }

    fn make_uniform_background(width: usize, height: usize, value: f32) -> Buffer2<f32> {
        Buffer2::new(width, height, vec![value; width * height])
    }

    #[test]
    fn test_matches_scalar_centered() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let scalar =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0).unwrap();
        let neon = unsafe {
            refine_centroid_neon(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0).unwrap()
        };

        assert!(
            (scalar.x - neon.x).abs() < 0.01,
            "NEON cx={} should match scalar cx={}",
            neon.x,
            scalar.x
        );
        assert!(
            (scalar.y - neon.y).abs() < 0.01,
            "NEON cy={} should match scalar cy={}",
            neon.y,
            scalar.y
        );
    }

    #[test]
    fn test_matches_scalar_offset() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let scalar =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0).unwrap();
        let neon = unsafe {
            refine_centroid_neon(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0).unwrap()
        };

        assert!(
            (scalar.x - neon.x).abs() < 0.01,
            "NEON cx={} should match scalar cx={}",
            neon.x,
            scalar.x
        );
        assert!(
            (scalar.y - neon.y).abs() < 0.01,
            "NEON cy={} should match scalar cy={}",
            neon.y,
            scalar.y
        );
    }

    #[test]
    fn test_invalid_position() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = unsafe {
            refine_centroid_neon(&pixels, width, height, &bg, Vec2::new(3.0, 32.0), 7, 4.0)
        };
        assert!(result.is_none());
    }

    #[test]
    fn test_zero_signal() {
        let width = 64;
        let height = 64;
        let pixels = make_uniform_background(width, height, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result =
            unsafe { refine_centroid_neon(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0) };
        assert!(result.is_none());
    }

    #[test]
    fn test_different_stamp_sizes() {
        let width = 128;
        let height = 128;
        let pixels = make_gaussian_star(width, height, Vec2::new(64.3, 64.7), 4.0, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        // Test various stamp radii that exercise different remainder handling
        // NEON processes 4 pixels at a time
        for stamp_radius in [4, 5, 6, 7, 8, 9, 10] {
            let scalar = refine_centroid_scalar(
                &pixels,
                width,
                height,
                &bg,
                Vec2::splat(64.0),
                stamp_radius,
                6.0,
            );
            let neon = unsafe {
                refine_centroid_neon(
                    &pixels,
                    width,
                    height,
                    &bg,
                    Vec2::splat(64.0),
                    stamp_radius,
                    6.0,
                )
            };

            match (scalar, neon) {
                (Some(s), Some(n)) => {
                    assert!(
                        (s.x - n.x).abs() < 0.01,
                        "stamp_radius={}: NEON cx={} should match scalar cx={}",
                        stamp_radius,
                        n.x,
                        s.x
                    );
                    assert!(
                        (s.y - n.y).abs() < 0.01,
                        "stamp_radius={}: NEON cy={} should match scalar cy={}",
                        stamp_radius,
                        n.y,
                        s.y
                    );
                }
                (None, None) => {}
                _ => panic!(
                    "stamp_radius={}: scalar and NEON should have same Some/None",
                    stamp_radius
                ),
            }
        }
    }

    #[test]
    fn test_consistency_multiple_positions() {
        let width = 128;
        let height = 128;

        let positions = [
            (40.1, 40.2),
            (64.5, 64.5),
            (80.7, 80.3),
            (50.0, 70.0),
            (70.0, 50.0),
        ];

        for (true_cx, true_cy) in positions {
            let true_pos = Vec2::new(true_cx, true_cy);
            let pixels = make_gaussian_star(width, height, true_pos, 3.0, 0.8, 0.1);
            let bg = crate::testing::estimate_background(&pixels, &Config::default());

            let start_pos = true_pos.round();

            let scalar = refine_centroid_scalar(&pixels, width, height, &bg, start_pos, 7, 5.0);
            let neon =
                unsafe { refine_centroid_neon(&pixels, width, height, &bg, start_pos, 7, 5.0) };

            let s = scalar.unwrap();
            let n = neon.unwrap();

            assert!(
                (s.x - n.x).abs() < 0.01,
                "pos=({}, {}): NEON cx={} should match scalar cx={}",
                true_cx,
                true_cy,
                n.x,
                s.x
            );
            assert!(
                (s.y - n.y).abs() < 0.01,
                "pos=({}, {}): NEON cy={} should match scalar cy={}",
                true_cx,
                true_cy,
                n.y,
                s.y
            );
        }
    }
}
