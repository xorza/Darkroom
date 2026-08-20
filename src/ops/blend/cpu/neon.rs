//! NEON blend kernels for `RGBA_U8` and `RGBA_F32`. Each blends one row pair
//! and finishes the sub-vector tail with the scalar reference, so the vector
//! body and the tail produce identical results.

use std::arch::aarch64::*;

use crate::ops::blend::cpu::BlendApply;
use crate::ops::blend::{Blend, BlendMode};

pub(super) unsafe fn rgba_u8_row(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    params: Blend,
) {
    let Blend { mode, alpha } = params;

    unsafe {
        let alpha_vec = vdupq_n_f32(alpha);
        let one_minus_alpha = vdupq_n_f32(1.0 - alpha);
        let one = vdupq_n_f32(1.0);
        let zero = vdupq_n_f32(0.0);
        let half = vdupq_n_f32(0.5);
        let two = vdupq_n_f32(2.0);
        let scale = vdupq_n_f32(255.0);
        let inv_scale = vdupq_n_f32(1.0 / 255.0);

        // Process 4 RGBA pixels at a time using deinterleaved loads
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 4 RGBA pixels deinterleaved (R, G, B, A separate)
            let src_pixels = vld4_u8(src_row[x * 4..].as_ptr());
            let dst_pixels = vld4_u8(dst_row[x * 4..].as_ptr());

            macro_rules! process_channel {
                ($src_chan:expr, $dst_chan:expr) => {{
                    // Convert to f32 (only first 4 values, we have 8 in uint8x8)
                    let src_16 = vmovl_u8($src_chan);
                    let dst_16 = vmovl_u8($dst_chan);

                    let src_32_lo = vmovl_u16(vget_low_u16(src_16));
                    let dst_32_lo = vmovl_u16(vget_low_u16(dst_16));

                    let src_f = vmulq_f32(vcvtq_f32_u32(src_32_lo), inv_scale);
                    let dst_f = vmulq_f32(vcvtq_f32_u32(dst_32_lo), inv_scale);

                    let blended = match mode {
                        BlendMode::Normal => src_f,
                        BlendMode::Add => vminq_f32(vaddq_f32(src_f, dst_f), one),
                        BlendMode::Subtract => vmaxq_f32(vsubq_f32(dst_f, src_f), zero),
                        BlendMode::Multiply => vmulq_f32(src_f, dst_f),
                        BlendMode::Screen => {
                            vsubq_f32(one, vmulq_f32(vsubq_f32(one, src_f), vsubq_f32(one, dst_f)))
                        }
                        BlendMode::Overlay => {
                            let mask = vcltq_f32(dst_f, half);
                            let dark = vmulq_f32(two, vmulq_f32(src_f, dst_f));
                            let light = vsubq_f32(
                                one,
                                vmulq_f32(
                                    two,
                                    vmulq_f32(vsubq_f32(one, src_f), vsubq_f32(one, dst_f)),
                                ),
                            );
                            vbslq_f32(mask, dark, light)
                        }
                    };

                    // Apply alpha: result = blended * alpha + dst * (1 - alpha)
                    let result = vmlaq_f32(vmulq_f32(dst_f, one_minus_alpha), blended, alpha_vec);

                    // Convert back to u8
                    let result_scaled = vmulq_f32(vminq_f32(vmaxq_f32(result, zero), one), scale);
                    let result_u32 = vcvtq_u32_f32(result_scaled);
                    let result_u16 = vmovn_u32(result_u32);
                    // We only have 4 values, pad with zeros for vmovn_u16
                    let result_u16_full = vcombine_u16(result_u16, vdup_n_u16(0));
                    vmovn_u16(result_u16_full)
                }};
            }

            // Process R, G, B channels with blend mode
            let r_out = process_channel!(src_pixels.0, dst_pixels.0);
            let g_out = process_channel!(src_pixels.1, dst_pixels.1);
            let b_out = process_channel!(src_pixels.2, dst_pixels.2);

            // Alpha uses normal blend (weighted average)
            let a_src_16 = vmovl_u8(src_pixels.3);
            let a_dst_16 = vmovl_u8(dst_pixels.3);
            let a_src_32 = vmovl_u16(vget_low_u16(a_src_16));
            let a_dst_32 = vmovl_u16(vget_low_u16(a_dst_16));
            let a_src_f = vmulq_f32(vcvtq_f32_u32(a_src_32), inv_scale);
            let a_dst_f = vmulq_f32(vcvtq_f32_u32(a_dst_32), inv_scale);
            let a_result = vmlaq_f32(vmulq_f32(a_dst_f, one_minus_alpha), a_src_f, alpha_vec);
            let a_scaled = vmulq_f32(vminq_f32(vmaxq_f32(a_result, zero), one), scale);
            let a_u32 = vcvtq_u32_f32(a_scaled);
            let a_u16 = vmovn_u32(a_u32);
            let a_u16_full = vcombine_u16(a_u16, vdup_n_u16(0));
            let a_out = vmovn_u16(a_u16_full);

            // Store interleaved - but we only have 4 pixels worth of data in the low half
            // Need to extract just the first 4 bytes from each channel
            let result = uint8x8x4_t(r_out, g_out, b_out, a_out);
            // Store only 16 bytes (4 RGBA pixels)
            vst4_lane_u8::<0>(out_row[x * 4..].as_mut_ptr(), result);
            vst4_lane_u8::<1>(out_row[x * 4 + 4..].as_mut_ptr(), result);
            vst4_lane_u8::<2>(out_row[x * 4 + 8..].as_mut_ptr(), result);
            vst4_lane_u8::<3>(out_row[x * 4 + 12..].as_mut_ptr(), result);

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

pub(super) unsafe fn rgba_f32_row(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    params: Blend,
) {
    let Blend { mode, alpha } = params;

    unsafe {
        let alpha_vec = vdupq_n_f32(alpha);
        let one_minus_alpha = vdupq_n_f32(1.0 - alpha);
        let one = vdupq_n_f32(1.0);
        let zero = vdupq_n_f32(0.0);
        let half = vdupq_n_f32(0.5);
        let two = vdupq_n_f32(2.0);

        let src_f32: &[f32] = bytemuck::cast_slice(src_row);
        let dst_f32: &[f32] = bytemuck::cast_slice(dst_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 4 RGBA pixels at a time using deinterleaved loads
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 4 RGBA pixels deinterleaved
            let src_pixels = vld4q_f32(src_f32[x * 4..].as_ptr());
            let dst_pixels = vld4q_f32(dst_f32[x * 4..].as_ptr());

            macro_rules! blend_channel {
                ($src:expr, $dst:expr) => {{
                    let blended = match mode {
                        BlendMode::Normal => $src,
                        BlendMode::Add => vminq_f32(vaddq_f32($src, $dst), one),
                        BlendMode::Subtract => vmaxq_f32(vsubq_f32($dst, $src), zero),
                        BlendMode::Multiply => vmulq_f32($src, $dst),
                        BlendMode::Screen => {
                            vsubq_f32(one, vmulq_f32(vsubq_f32(one, $src), vsubq_f32(one, $dst)))
                        }
                        BlendMode::Overlay => {
                            let mask = vcltq_f32($dst, half);
                            let dark = vmulq_f32(two, vmulq_f32($src, $dst));
                            let light = vsubq_f32(
                                one,
                                vmulq_f32(
                                    two,
                                    vmulq_f32(vsubq_f32(one, $src), vsubq_f32(one, $dst)),
                                ),
                            );
                            vbslq_f32(mask, dark, light)
                        }
                    };
                    // result = blended * alpha + dst * (1 - alpha)
                    let result = vmlaq_f32(vmulq_f32($dst, one_minus_alpha), blended, alpha_vec);
                    vminq_f32(vmaxq_f32(result, zero), one)
                }};
            }

            let r_out = blend_channel!(src_pixels.0, dst_pixels.0);
            let g_out = blend_channel!(src_pixels.1, dst_pixels.1);
            let b_out = blend_channel!(src_pixels.2, dst_pixels.2);
            // Alpha uses normal blend
            let a_blended = src_pixels.3;
            let a_result = vmlaq_f32(
                vmulq_f32(dst_pixels.3, one_minus_alpha),
                a_blended,
                alpha_vec,
            );
            let a_out = vminq_f32(vmaxq_f32(a_result, zero), one);

            let result = float32x4x4_t(r_out, g_out, b_out, a_out);
            vst4q_f32(out_f32[x * 4..].as_mut_ptr(), result);

            x += simd_width;
        }

        // Sub-vector tail through the scalar reference, so it cannot disagree
        // with the vector body it follows.
        while x < width {
            for c in 0..3 {
                out_f32[x * 4 + c] = src_f32[x * 4 + c].blend(dst_f32[x * 4 + c], mode, alpha);
            }
            out_f32[x * 4 + 3] =
                src_f32[x * 4 + 3].blend(dst_f32[x * 4 + 3], BlendMode::Normal, alpha);
            x += 1;
        }
    }
}
