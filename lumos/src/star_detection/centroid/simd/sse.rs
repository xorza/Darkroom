//! SSE SIMD implementation of centroid refinement.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use glam::Vec2;

use crate::math::{FWHM_TO_SIGMA, fast_exp};
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::centroid::is_valid_stamp_position;

/// SSE4.1 SIMD implementation of centroid refinement.
///
/// Processes 4 pixels at a time using 128-bit SIMD registers.
/// Fallback for systems without AVX2 support.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub unsafe fn refine_centroid_sse(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
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
        let a_vec = _mm_set1_ps(12102203.0);
        let b_vec = _mm_set1_epi32(1065353216 - 486411);
        let neg_inv_two_sigma_sq_vec = _mm_set1_ps(neg_inv_two_sigma_sq);
        let cx_vec = _mm_set1_ps(cx);
        let zero_vec = _mm_setzero_ps();

        let mut sum_x_vec = _mm_setzero_ps();
        let mut sum_y_vec = _mm_setzero_ps();
        let mut sum_w_vec = _mm_setzero_ps();

        let stamp_radius_i32 = stamp_radius as i32;
        let stamp_width = (2 * stamp_radius + 1) as i32;

        for dy in -stamp_radius_i32..=stamp_radius_i32 {
            let y = (icy + dy as isize) as usize;
            let fy = y as f32 - cy;
            let fy_sq = fy * fy;
            let fy_sq_vec = _mm_set1_ps(fy_sq);
            let y_vec = _mm_set1_ps(y as f32);

            let row_start_x = (icx - stamp_radius_i32 as isize) as usize;
            let row_idx = y * width + row_start_x;

            // Process 4 pixels at a time
            let mut dx = 0i32;
            while dx + 4 <= stamp_width {
                let base_x = row_start_x + dx as usize;

                // Load 4 pixel values and backgrounds
                let pix_ptr = pixels.as_ptr().add(row_idx + dx as usize);
                let bg_ptr = background.background.as_ptr().add(row_idx + dx as usize);
                let pix_vec = _mm_loadu_ps(pix_ptr);
                let bg_vec = _mm_loadu_ps(bg_ptr);

                // Background-subtracted value, clamped to >= 0
                let value_vec = _mm_max_ps(_mm_sub_ps(pix_vec, bg_vec), zero_vec);

                // X coordinates for these 4 pixels
                let x_offsets = _mm_set_ps(3.0, 2.0, 1.0, 0.0);
                let x_vec = _mm_add_ps(_mm_set1_ps(base_x as f32), x_offsets);

                // Distance squared: (x - cx)² + (y - cy)²
                let fx_vec = _mm_sub_ps(x_vec, cx_vec);
                let fx_sq_vec = _mm_mul_ps(fx_vec, fx_vec);
                let dist_sq_vec = _mm_add_ps(fx_sq_vec, fy_sq_vec);

                // Gaussian weight: exp(-dist_sq / two_sigma_sq)
                // Using Schraudolph's fast exp: reinterpret(A * x + B) as f32
                let exp_arg = _mm_mul_ps(dist_sq_vec, neg_inv_two_sigma_sq_vec);
                let exp_scaled = _mm_mul_ps(exp_arg, a_vec);
                let exp_int = _mm_add_epi32(_mm_cvtps_epi32(exp_scaled), b_vec);
                let gauss_weight = _mm_castsi128_ps(exp_int);

                // Weight = value * gauss_weight
                let weight_vec = _mm_mul_ps(value_vec, gauss_weight);

                // Accumulate weighted sums (no FMA in SSE4.1, use mul + add)
                sum_x_vec = _mm_add_ps(sum_x_vec, _mm_mul_ps(x_vec, weight_vec));
                sum_y_vec = _mm_add_ps(sum_y_vec, _mm_mul_ps(y_vec, weight_vec));
                sum_w_vec = _mm_add_ps(sum_w_vec, weight_vec);

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
                sum_x_vec = _mm_add_ps(sum_x_vec, _mm_set_ps(0.0, 0.0, 0.0, x as f32 * weight));
                sum_y_vec = _mm_add_ps(sum_y_vec, _mm_set_ps(0.0, 0.0, 0.0, y as f32 * weight));
                sum_w_vec = _mm_add_ps(sum_w_vec, _mm_set_ps(0.0, 0.0, 0.0, weight));

                dx += 1;
            }
        }

        // Horizontal sum of SIMD accumulators
        (
            horizontal_sum_128(sum_x_vec),
            horizontal_sum_128(sum_y_vec),
            horizontal_sum_128(sum_w_vec),
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

/// Horizontal sum of a 128-bit f32 vector (4 elements).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn horizontal_sum_128(v: __m128) -> f32 {
    let shuf = _mm_movehdup_ps(v);
    let sums = _mm_add_ps(v, shuf);
    let shuf = _mm_movehl_ps(sums, sums);
    let sums = _mm_add_ss(sums, shuf);
    _mm_cvtss_f32(sums)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Buffer2;
    use crate::star_detection::background::BackgroundConfig;
    use crate::star_detection::centroid::simd::refine_centroid_scalar;
    use common::cpu_features;
    use glam::Vec2;

    fn make_gaussian_star(
        width: usize,
        height: usize,
        cx: f32,
        cy: f32,
        sigma: f32,
        amplitude: f32,
        background: f32,
    ) -> Buffer2<f32> {
        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
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

    fn run_if_sse41<F: FnOnce()>(f: F) {
        if cpu_features::has_sse4_1() {
            f();
        } else {
            eprintln!("Skipping SSE4.1 test: CPU does not support SSE4.1");
        }
    }

    #[test]
    fn test_matches_scalar_centered() {
        run_if_sse41(|| {
            let width = 64;
            let height = 64;
            let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8, 0.1);
            let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());

            let scalar =
                refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0)
                    .unwrap();
            let sse = unsafe {
                refine_centroid_sse(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0).unwrap()
            };

            assert!(
                (scalar.x - sse.x).abs() < 0.01,
                "SSE cx={} should match scalar cx={}",
                sse.x,
                scalar.x
            );
            assert!(
                (scalar.y - sse.y).abs() < 0.01,
                "SSE cy={} should match scalar cy={}",
                sse.y,
                scalar.y
            );
        });
    }

    #[test]
    fn test_matches_scalar_offset() {
        run_if_sse41(|| {
            let width = 64;
            let height = 64;
            let pixels = make_gaussian_star(width, height, 32.3, 32.7, 2.5, 0.8, 0.1);
            let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());

            let scalar =
                refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0)
                    .unwrap();
            let sse = unsafe {
                refine_centroid_sse(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0).unwrap()
            };

            assert!(
                (scalar.x - sse.x).abs() < 0.01,
                "SSE cx={} should match scalar cx={}",
                sse.x,
                scalar.x
            );
            assert!(
                (scalar.y - sse.y).abs() < 0.01,
                "SSE cy={} should match scalar cy={}",
                sse.y,
                scalar.y
            );
        });
    }

    #[test]
    fn test_invalid_position() {
        run_if_sse41(|| {
            let width = 64;
            let height = 64;
            let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8, 0.1);
            let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());

            let result = unsafe {
                refine_centroid_sse(&pixels, width, height, &bg, Vec2::new(3.0, 32.0), 7, 4.0)
            };
            assert!(result.is_none());
        });
    }

    #[test]
    fn test_zero_signal() {
        run_if_sse41(|| {
            let width = 64;
            let height = 64;
            let pixels = make_uniform_background(width, height, 0.1);
            let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());

            let result = unsafe {
                refine_centroid_sse(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0)
            };
            assert!(result.is_none());
        });
    }

    #[test]
    fn test_different_stamp_sizes() {
        run_if_sse41(|| {
            let width = 128;
            let height = 128;
            let pixels = make_gaussian_star(width, height, 64.3, 64.7, 4.0, 0.8, 0.1);
            let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());

            // Test various stamp radii that exercise different remainder handling
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
                let sse = unsafe {
                    refine_centroid_sse(
                        &pixels,
                        width,
                        height,
                        &bg,
                        Vec2::splat(64.0),
                        stamp_radius,
                        6.0,
                    )
                };

                match (scalar, sse) {
                    (Some(s), Some(e)) => {
                        assert!(
                            (s.x - e.x).abs() < 0.01,
                            "stamp_radius={}: SSE cx={} should match scalar cx={}",
                            stamp_radius,
                            e.x,
                            s.x
                        );
                        assert!(
                            (s.y - e.y).abs() < 0.01,
                            "stamp_radius={}: SSE cy={} should match scalar cy={}",
                            stamp_radius,
                            e.y,
                            s.y
                        );
                    }
                    (None, None) => {}
                    _ => panic!(
                        "stamp_radius={}: scalar and SSE should have same Some/None",
                        stamp_radius
                    ),
                }
            }
        });
    }

    #[test]
    fn test_consistency_multiple_positions() {
        run_if_sse41(|| {
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
                let pixels = make_gaussian_star(width, height, true_cx, true_cy, 3.0, 0.8, 0.1);
                let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());

                let start_pos = Vec2::new(true_cx.round(), true_cy.round());

                let scalar = refine_centroid_scalar(&pixels, width, height, &bg, start_pos, 7, 5.0);
                let sse =
                    unsafe { refine_centroid_sse(&pixels, width, height, &bg, start_pos, 7, 5.0) };

                let s = scalar.unwrap();
                let e = sse.unwrap();

                assert!(
                    (s.x - e.x).abs() < 0.01,
                    "pos=({}, {}): SSE cx={} should match scalar cx={}",
                    true_cx,
                    true_cy,
                    e.x,
                    s.x
                );
                assert!(
                    (s.y - e.y).abs() < 0.01,
                    "pos=({}, {}): SSE cy={} should match scalar cy={}",
                    true_cx,
                    true_cy,
                    e.y,
                    s.y
                );
            }
        });
    }
}
