//! SSE4.1 blend kernels for `RGBA_U8` and `RGBA_F32`. Each blends one row pair
//! and finishes the sub-vector tail with the scalar reference, so the vector
//! body and the tail produce identical results.

use std::arch::x86_64::*;

use crate::ops::blend::cpu::BlendApply;
use crate::ops::blend::{Blend, BlendMode};

#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn rgba_u8_row(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    params: Blend,
) {
    let Blend { mode, alpha } = params;

    unsafe {
        let alpha_vec = _mm_set1_ps(alpha);
        let one_minus_alpha = _mm_set1_ps(1.0 - alpha);
        let scale = _mm_set1_ps(255.0);
        let one = _mm_set1_ps(1.0);
        let zero = _mm_setzero_ps();
        let half = _mm_set1_ps(0.5);
        let two = _mm_set1_ps(2.0);

        // Process 4 RGBA pixels at a time
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Process each pixel separately (unpack, blend, repack)
            // This is simpler than trying to do 4 pixels in parallel with complex blend modes
            let mut result_bytes = [0u8; 16];

            for i in 0..4 {
                let src_offset = i * 4;
                let src_r = _mm_set1_ps(src_row[x * 4 + src_offset] as f32 * (1.0 / 255.0));
                let src_g = _mm_set1_ps(src_row[x * 4 + src_offset + 1] as f32 * (1.0 / 255.0));
                let src_b = _mm_set1_ps(src_row[x * 4 + src_offset + 2] as f32 * (1.0 / 255.0));
                let src_a = _mm_set1_ps(src_row[x * 4 + src_offset + 3] as f32 * (1.0 / 255.0));

                let dst_r = _mm_set1_ps(dst_row[x * 4 + src_offset] as f32 * (1.0 / 255.0));
                let dst_g = _mm_set1_ps(dst_row[x * 4 + src_offset + 1] as f32 * (1.0 / 255.0));
                let dst_b = _mm_set1_ps(dst_row[x * 4 + src_offset + 2] as f32 * (1.0 / 255.0));
                let dst_a = _mm_set1_ps(dst_row[x * 4 + src_offset + 3] as f32 * (1.0 / 255.0));

                macro_rules! blend_channel {
                    ($src:expr, $dst:expr) => {{
                        let blended = match mode {
                            BlendMode::Normal => $src,
                            BlendMode::Add => _mm_min_ps(_mm_add_ps($src, $dst), one),
                            BlendMode::Subtract => _mm_max_ps(_mm_sub_ps($dst, $src), zero),
                            BlendMode::Multiply => _mm_mul_ps($src, $dst),
                            BlendMode::Screen => _mm_sub_ps(
                                one,
                                _mm_mul_ps(_mm_sub_ps(one, $src), _mm_sub_ps(one, $dst)),
                            ),
                            BlendMode::Overlay => {
                                let mask = _mm_cmplt_ps($dst, half);
                                let dark = _mm_mul_ps(two, _mm_mul_ps($src, $dst));
                                let light = _mm_sub_ps(
                                    one,
                                    _mm_mul_ps(
                                        two,
                                        _mm_mul_ps(_mm_sub_ps(one, $src), _mm_sub_ps(one, $dst)),
                                    ),
                                );
                                _mm_blendv_ps(light, dark, mask)
                            }
                        };
                        _mm_add_ps(
                            _mm_mul_ps(blended, alpha_vec),
                            _mm_mul_ps($dst, one_minus_alpha),
                        )
                    }};
                }

                let out_r = blend_channel!(src_r, dst_r);
                let out_g = blend_channel!(src_g, dst_g);
                let out_b = blend_channel!(src_b, dst_b);
                // Alpha uses normal blend
                let out_a = _mm_add_ps(
                    _mm_mul_ps(src_a, alpha_vec),
                    _mm_mul_ps(dst_a, one_minus_alpha),
                );

                // Convert back to u8
                result_bytes[i * 4] =
                    (_mm_cvtss_f32(_mm_mul_ps(out_r, scale)).clamp(0.0, 255.0)) as u8;
                result_bytes[i * 4 + 1] =
                    (_mm_cvtss_f32(_mm_mul_ps(out_g, scale)).clamp(0.0, 255.0)) as u8;
                result_bytes[i * 4 + 2] =
                    (_mm_cvtss_f32(_mm_mul_ps(out_b, scale)).clamp(0.0, 255.0)) as u8;
                result_bytes[i * 4 + 3] =
                    (_mm_cvtss_f32(_mm_mul_ps(out_a, scale)).clamp(0.0, 255.0)) as u8;
            }

            let result = _mm_loadu_si128(result_bytes.as_ptr() as *const __m128i);
            _mm_storeu_si128(out_row[x * 4..].as_mut_ptr() as *mut __m128i, result);

            x += simd_width;
        }

        // Sub-vector tail through the scalar reference, so it cannot disagree
        // with the vector body it follows.
        while x < width {
            for c in 0..3 {
                out_row[x * 4 + c] = src_row[x * 4 + c].blend(dst_row[x * 4 + c], mode, alpha);
            }
            out_row[x * 4 + 3] =
                src_row[x * 4 + 3].blend(dst_row[x * 4 + 3], BlendMode::Normal, alpha);
            x += 1;
        }
    }
}

#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn rgba_f32_row(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    params: Blend,
) {
    let Blend { mode, alpha } = params;

    unsafe {
        let alpha_vec = _mm_set1_ps(alpha);
        let one_minus_alpha = _mm_set1_ps(1.0 - alpha);
        let one = _mm_set1_ps(1.0);
        let zero = _mm_setzero_ps();
        let half = _mm_set1_ps(0.5);
        let two = _mm_set1_ps(2.0);

        let src_f32: &[f32] = bytemuck::cast_slice(src_row);
        let dst_f32: &[f32] = bytemuck::cast_slice(dst_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 1 RGBA pixel at a time (4 floats fit in one SSE register)
        let mut x = 0;

        while x < width {
            let src_pixel = _mm_loadu_ps(src_f32[x * 4..].as_ptr());
            let dst_pixel = _mm_loadu_ps(dst_f32[x * 4..].as_ptr());

            let blended = match mode {
                BlendMode::Normal => src_pixel,
                BlendMode::Add => _mm_min_ps(_mm_add_ps(src_pixel, dst_pixel), one),
                BlendMode::Subtract => _mm_max_ps(_mm_sub_ps(dst_pixel, src_pixel), zero),
                BlendMode::Multiply => _mm_mul_ps(src_pixel, dst_pixel),
                BlendMode::Screen => _mm_sub_ps(
                    one,
                    _mm_mul_ps(_mm_sub_ps(one, src_pixel), _mm_sub_ps(one, dst_pixel)),
                ),
                BlendMode::Overlay => {
                    let mask = _mm_cmplt_ps(dst_pixel, half);
                    let dark = _mm_mul_ps(two, _mm_mul_ps(src_pixel, dst_pixel));
                    let light = _mm_sub_ps(
                        one,
                        _mm_mul_ps(
                            two,
                            _mm_mul_ps(_mm_sub_ps(one, src_pixel), _mm_sub_ps(one, dst_pixel)),
                        ),
                    );
                    _mm_blendv_ps(light, dark, mask)
                }
            };

            // Lane 3 is alpha, which carries no blend mode of its own: restoring
            // src there leaves it the plain alpha mix the scalar reference does.
            let blended = _mm_blend_ps::<0b1000>(blended, src_pixel);

            let result = _mm_add_ps(
                _mm_mul_ps(blended, alpha_vec),
                _mm_mul_ps(dst_pixel, one_minus_alpha),
            );
            let clamped = _mm_min_ps(_mm_max_ps(result, zero), one);

            _mm_storeu_ps(out_f32[x * 4..].as_mut_ptr(), clamped);
            x += 1;
        }
    }
}
