//! x86_64 SSE3 SIMD implementation of bilinear demosaicing.

use super::BayerImage;
use super::scalar::process_pixel_scalar;

#[target_feature(enable = "sse3")]
pub(crate) unsafe fn demosaic_bilinear_sse3(bayer: &BayerImage) -> Vec<f32> {
    use std::arch::x86_64::*;

    let mut rgb = vec![0.0f32; bayer.width * bayer.height * 3];
    let half = _mm_set1_ps(0.5);
    let quarter = _mm_set1_ps(0.25);

    unsafe {
        // Process interior pixels with SIMD (skip 1-pixel border for safe neighbor access)
        for y in 1..bayer.height.saturating_sub(1) {
            let raw_y = y + bayer.top_margin;
            let row_above = (raw_y - 1) * bayer.raw_width;
            let row_current = raw_y * bayer.raw_width;
            let row_below = (raw_y + 1) * bayer.raw_width;

            // Process 4 pixels at a time (2 pairs of RGGB/GRBG patterns)
            let mut x = 1;
            while x + 4 <= bayer.width.saturating_sub(1) {
                let raw_x = x + bayer.left_margin;

                // Load 4 consecutive pixels and their neighbors
                let center = _mm_loadu_ps(bayer.data.as_ptr().add(row_current + raw_x));
                let left = _mm_loadu_ps(bayer.data.as_ptr().add(row_current + raw_x - 1));
                let right = _mm_loadu_ps(bayer.data.as_ptr().add(row_current + raw_x + 1));
                let top = _mm_loadu_ps(bayer.data.as_ptr().add(row_above + raw_x));
                let bottom = _mm_loadu_ps(bayer.data.as_ptr().add(row_below + raw_x));

                // Diagonal neighbors
                let top_left = _mm_loadu_ps(bayer.data.as_ptr().add(row_above + raw_x - 1));
                let top_right = _mm_loadu_ps(bayer.data.as_ptr().add(row_above + raw_x + 1));
                let bottom_left = _mm_loadu_ps(bayer.data.as_ptr().add(row_below + raw_x - 1));
                let bottom_right = _mm_loadu_ps(bayer.data.as_ptr().add(row_below + raw_x + 1));

                // Compute interpolations
                let h_interp = _mm_mul_ps(_mm_add_ps(left, right), half);
                let v_interp = _mm_mul_ps(_mm_add_ps(top, bottom), half);
                let cross_interp = _mm_mul_ps(
                    _mm_add_ps(_mm_add_ps(left, right), _mm_add_ps(top, bottom)),
                    quarter,
                );
                let diag_interp = _mm_mul_ps(
                    _mm_add_ps(
                        _mm_add_ps(top_left, top_right),
                        _mm_add_ps(bottom_left, bottom_right),
                    ),
                    quarter,
                );

                // Extract values and assign based on CFA pattern
                let center_arr: [f32; 4] = std::mem::transmute(center);
                let h_arr: [f32; 4] = std::mem::transmute(h_interp);
                let v_arr: [f32; 4] = std::mem::transmute(v_interp);
                let cross_arr: [f32; 4] = std::mem::transmute(cross_interp);
                let diag_arr: [f32; 4] = std::mem::transmute(diag_interp);

                for i in 0..4 {
                    let px = x + i;
                    let raw_px = raw_x + i;
                    let color = bayer.cfa.color_at(raw_y, raw_px);
                    let rgb_idx = (y * bayer.width + px) * 3;

                    match color {
                        0 => {
                            // Red pixel
                            rgb[rgb_idx] = center_arr[i];
                            rgb[rgb_idx + 1] = cross_arr[i];
                            rgb[rgb_idx + 2] = diag_arr[i];
                        }
                        1 => {
                            // Green pixel
                            if bayer.cfa.red_in_row(raw_y) {
                                rgb[rgb_idx] = h_arr[i];
                                rgb[rgb_idx + 2] = v_arr[i];
                            } else {
                                rgb[rgb_idx] = v_arr[i];
                                rgb[rgb_idx + 2] = h_arr[i];
                            }
                            rgb[rgb_idx + 1] = center_arr[i];
                        }
                        2 => {
                            // Blue pixel
                            rgb[rgb_idx] = diag_arr[i];
                            rgb[rgb_idx + 1] = cross_arr[i];
                            rgb[rgb_idx + 2] = center_arr[i];
                        }
                        _ => unreachable!(),
                    }
                }

                x += 4;
            }

            // Handle remaining pixels in this row with scalar code
            while x < bayer.width {
                let raw_x = x + bayer.left_margin;
                process_pixel_scalar(bayer, &mut rgb, x, y, raw_x, raw_y);
                x += 1;
            }
        }

        // Process border rows with scalar code
        for y in 0..bayer.height {
            if y == 0 || y == bayer.height - 1 || y >= bayer.height.saturating_sub(1) {
                for x in 0..bayer.width {
                    let raw_y = y + bayer.top_margin;
                    let raw_x = x + bayer.left_margin;
                    process_pixel_scalar(bayer, &mut rgb, x, y, raw_x, raw_y);
                }
            } else {
                // Process left and right border pixels
                let raw_y = y + bayer.top_margin;
                if bayer.width > 0 {
                    process_pixel_scalar(bayer, &mut rgb, 0, y, bayer.left_margin, raw_y);
                }
                if bayer.width > 1 {
                    let last_x = bayer.width - 1;
                    process_pixel_scalar(
                        bayer,
                        &mut rgb,
                        last_x,
                        y,
                        last_x + bayer.left_margin,
                        raw_y,
                    );
                }
            }
        }
    }

    rgb
}
