//! x86_64 SSE3 SIMD implementation of bilinear demosaicing.

use super::BayerImage;
use super::scalar;

#[target_feature(enable = "sse3")]
pub(crate) unsafe fn demosaic_bilinear_sse3(bayer: &BayerImage) -> Vec<f32> {
    use std::arch::x86_64::*;

    let mut rgb = vec![0.0f32; bayer.width * bayer.height * 3];

    // Pre-compute pattern lookup
    let pattern = bayer.cfa.pattern_2x2();

    unsafe {
        let half = _mm_set1_ps(0.5);
        let quarter = _mm_set1_ps(0.25);

        // Process interior pixels with SIMD (skip 1-pixel border for safe neighbor access)
        for y in 0..bayer.height {
            let raw_y = y + bayer.top_margin;
            let red_in_row = bayer.cfa.red_in_row(raw_y);
            let row_pattern_idx = (raw_y & 1) << 1;

            // Check if we can use SIMD for this row
            let can_simd = raw_y > 0
                && raw_y + 1 < bayer.raw_height
                && bayer.left_margin > 0
                && bayer.width >= 8;

            if !can_simd {
                // Process entire row with scalar
                for x in 0..bayer.width {
                    let raw_x = x + bayer.left_margin;
                    let color = pattern[row_pattern_idx | (raw_x & 1)];
                    let rgb_idx = (y * bayer.width + x) * 3;
                    let val = bayer.data[raw_y * bayer.raw_width + raw_x];

                    match color {
                        0 => {
                            rgb[rgb_idx] = val;
                            rgb[rgb_idx + 1] = scalar::interpolate_cross(bayer, raw_x, raw_y);
                            rgb[rgb_idx + 2] = scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                        }
                        1 => {
                            if red_in_row {
                                rgb[rgb_idx] = scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                                rgb[rgb_idx + 2] =
                                    scalar::interpolate_vertical(bayer, raw_x, raw_y);
                            } else {
                                rgb[rgb_idx] = scalar::interpolate_vertical(bayer, raw_x, raw_y);
                                rgb[rgb_idx + 2] =
                                    scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                            }
                            rgb[rgb_idx + 1] = val;
                        }
                        2 => {
                            rgb[rgb_idx] = scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                            rgb[rgb_idx + 1] = scalar::interpolate_cross(bayer, raw_x, raw_y);
                            rgb[rgb_idx + 2] = val;
                        }
                        _ => unreachable!(),
                    }
                }
                continue;
            }

            let row_above = (raw_y - 1) * bayer.raw_width;
            let row_current = raw_y * bayer.raw_width;
            let row_below = (raw_y + 1) * bayer.raw_width;

            // Process left border pixel with scalar
            {
                let x = 0;
                let raw_x = bayer.left_margin;
                let color = pattern[row_pattern_idx | (raw_x & 1)];
                let rgb_idx = (y * bayer.width + x) * 3;
                let val = bayer.data[raw_y * bayer.raw_width + raw_x];

                match color {
                    0 => {
                        rgb[rgb_idx] = val;
                        rgb[rgb_idx + 1] = scalar::interpolate_cross(bayer, raw_x, raw_y);
                        rgb[rgb_idx + 2] = scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                    }
                    1 => {
                        if red_in_row {
                            rgb[rgb_idx] = scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                            rgb[rgb_idx + 2] = scalar::interpolate_vertical(bayer, raw_x, raw_y);
                        } else {
                            rgb[rgb_idx] = scalar::interpolate_vertical(bayer, raw_x, raw_y);
                            rgb[rgb_idx + 2] = scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                        }
                        rgb[rgb_idx + 1] = val;
                    }
                    2 => {
                        rgb[rgb_idx] = scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                        rgb[rgb_idx + 1] = scalar::interpolate_cross(bayer, raw_x, raw_y);
                        rgb[rgb_idx + 2] = val;
                    }
                    _ => unreachable!(),
                }
            }

            // Process interior with SIMD (4 pixels at a time)
            let mut x = 1;
            let simd_end = bayer.width.saturating_sub(1);

            while x + 4 <= simd_end {
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

                // Compute all interpolations in SIMD
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

                // Extract values for assignment
                let center_arr: [f32; 4] = std::mem::transmute(center);
                let h_arr: [f32; 4] = std::mem::transmute(h_interp);
                let v_arr: [f32; 4] = std::mem::transmute(v_interp);
                let cross_arr: [f32; 4] = std::mem::transmute(cross_interp);
                let diag_arr: [f32; 4] = std::mem::transmute(diag_interp);

                // Assign pixels using pre-computed pattern
                for i in 0..4 {
                    let px = x + i;
                    let raw_px = raw_x + i;
                    let rgb_idx = (y * bayer.width + px) * 3;
                    let color = pattern[row_pattern_idx | (raw_px & 1)];

                    match color {
                        0 => {
                            // Red pixel
                            rgb[rgb_idx] = center_arr[i];
                            rgb[rgb_idx + 1] = cross_arr[i];
                            rgb[rgb_idx + 2] = diag_arr[i];
                        }
                        1 => {
                            // Green pixel
                            if red_in_row {
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

            // Handle remaining pixels with scalar code
            while x < bayer.width {
                let raw_x = x + bayer.left_margin;
                let color = pattern[row_pattern_idx | (raw_x & 1)];
                let rgb_idx = (y * bayer.width + x) * 3;
                let val = bayer.data[raw_y * bayer.raw_width + raw_x];

                match color {
                    0 => {
                        rgb[rgb_idx] = val;
                        rgb[rgb_idx + 1] = scalar::interpolate_cross(bayer, raw_x, raw_y);
                        rgb[rgb_idx + 2] = scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                    }
                    1 => {
                        if red_in_row {
                            rgb[rgb_idx] = scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                            rgb[rgb_idx + 2] = scalar::interpolate_vertical(bayer, raw_x, raw_y);
                        } else {
                            rgb[rgb_idx] = scalar::interpolate_vertical(bayer, raw_x, raw_y);
                            rgb[rgb_idx + 2] = scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                        }
                        rgb[rgb_idx + 1] = val;
                    }
                    2 => {
                        rgb[rgb_idx] = scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                        rgb[rgb_idx + 1] = scalar::interpolate_cross(bayer, raw_x, raw_y);
                        rgb[rgb_idx + 2] = val;
                    }
                    _ => unreachable!(),
                }
                x += 1;
            }
        }
    }

    rgb
}
