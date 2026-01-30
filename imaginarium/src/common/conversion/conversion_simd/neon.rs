// NEON implementations for row conversion
//
// This module contains NEON SIMD implementations for aarch64.

#![allow(unsafe_op_in_unsafe_fn)]

use super::LUMA_B;
use super::LUMA_G;
use super::LUMA_R;

// =============================================================================
// NEON implementations (aarch64)
// =============================================================================

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_rgba_to_rgb_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 48;

        // Load 16 RGBA pixels deinterleaved
        let rgba = vld4q_u8(src_ptr.add(src_offset));

        // Store as RGB (interleaved)
        let rgb = uint8x16x3_t(rgba.0, rgba.1, rgba.2);
        vst3q_u8(dst_ptr.add(dst_offset), rgb);
    }

    let src_rem = &src[simd_width * 64..];
    let dst_rem = &mut dst[simd_width * 48..];
    for i in 0..remainder {
        dst_rem[i * 3] = src_rem[i * 4];
        dst_rem[i * 3 + 1] = src_rem[i * 4 + 1];
        dst_rem[i * 3 + 2] = src_rem[i * 4 + 2];
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_rgb_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let alpha = vdupq_n_u8(255);

    for i in 0..simd_width {
        let src_offset = i * 48;
        let dst_offset = i * 64;

        // Load 16 RGB pixels deinterleaved
        let rgb = vld3q_u8(src_ptr.add(src_offset));

        // Store as RGBA
        let rgba = uint8x16x4_t(rgb.0, rgb.1, rgb.2, alpha);
        vst4q_u8(dst_ptr.add(dst_offset), rgba);
    }

    let src_rem = &src[simd_width * 48..];
    let dst_rem = &mut dst[simd_width * 64..];
    for i in 0..remainder {
        dst_rem[i * 4] = src_rem[i * 3];
        dst_rem[i * 4 + 1] = src_rem[i * 3 + 1];
        dst_rem[i * 4 + 2] = src_rem[i * 3 + 2];
        dst_rem[i * 4 + 3] = 255;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_rgba_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let r_w = vdupq_n_u16(54);
    let g_w = vdupq_n_u16(183);
    let b_w = vdupq_n_u16(19);

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 16;

        let rgba = vld4q_u8(src_ptr.add(src_offset));
        let r = rgba.0;
        let g = rgba.1;
        let b = rgba.2;

        let r_lo = vmovl_u8(vget_low_u8(r));
        let r_hi = vmovl_u8(vget_high_u8(r));
        let g_lo = vmovl_u8(vget_low_u8(g));
        let g_hi = vmovl_u8(vget_high_u8(g));
        let b_lo = vmovl_u8(vget_low_u8(b));
        let b_hi = vmovl_u8(vget_high_u8(b));

        let sum_lo = vaddq_u16(
            vaddq_u16(vmulq_u16(r_lo, r_w), vmulq_u16(g_lo, g_w)),
            vmulq_u16(b_lo, b_w),
        );
        let sum_hi = vaddq_u16(
            vaddq_u16(vmulq_u16(r_hi, r_w), vmulq_u16(g_hi, g_w)),
            vmulq_u16(b_hi, b_w),
        );

        let lum_lo = vshrn_n_u16(sum_lo, 8);
        let lum_hi = vshrn_n_u16(sum_hi, 8);
        let lum = vcombine_u8(lum_lo, lum_hi);

        vst1q_u8(dst_ptr.add(dst_offset), lum);
    }

    let src_rem = &src[simd_width * 64..];
    let dst_rem = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_rem[i * 4] as u32;
        let g = src_rem[i * 4 + 1] as u32;
        let b = src_rem[i * 4 + 2] as u32;
        dst_rem[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_rgb_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let r_w = vdupq_n_u16(54);
    let g_w = vdupq_n_u16(183);
    let b_w = vdupq_n_u16(19);

    for i in 0..simd_width {
        let src_offset = i * 48;
        let dst_offset = i * 16;

        let rgb = vld3q_u8(src_ptr.add(src_offset));
        let r = rgb.0;
        let g = rgb.1;
        let b = rgb.2;

        let r_lo = vmovl_u8(vget_low_u8(r));
        let r_hi = vmovl_u8(vget_high_u8(r));
        let g_lo = vmovl_u8(vget_low_u8(g));
        let g_hi = vmovl_u8(vget_high_u8(g));
        let b_lo = vmovl_u8(vget_low_u8(b));
        let b_hi = vmovl_u8(vget_high_u8(b));

        let sum_lo = vaddq_u16(
            vaddq_u16(vmulq_u16(r_lo, r_w), vmulq_u16(g_lo, g_w)),
            vmulq_u16(b_lo, b_w),
        );
        let sum_hi = vaddq_u16(
            vaddq_u16(vmulq_u16(r_hi, r_w), vmulq_u16(g_hi, g_w)),
            vmulq_u16(b_hi, b_w),
        );

        let lum_lo = vshrn_n_u16(sum_lo, 8);
        let lum_hi = vshrn_n_u16(sum_hi, 8);
        let lum = vcombine_u8(lum_lo, lum_hi);

        vst1q_u8(dst_ptr.add(dst_offset), lum);
    }

    let src_rem = &src[simd_width * 48..];
    let dst_rem = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_rem[i * 3] as u32;
        let g = src_rem[i * 3 + 1] as u32;
        let b = src_rem[i * 3 + 2] as u32;
        dst_rem[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_l_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let alpha = vdupq_n_u8(255);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 64;

        let l = vld1q_u8(src_ptr.add(src_offset));
        let rgba = uint8x16x4_t(l, l, l, alpha);
        vst4q_u8(dst_ptr.add(dst_offset), rgba);
    }

    let src_rem = &src[simd_width * 16..];
    let dst_rem = &mut dst[simd_width * 64..];
    for i in 0..remainder {
        let l = src_rem[i];
        dst_rem[i * 4] = l;
        dst_rem[i * 4 + 1] = l;
        dst_rem[i * 4 + 2] = l;
        dst_rem[i * 4 + 3] = 255;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_l_to_rgb_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 48;

        let l = vld1q_u8(src_ptr.add(src_offset));
        let rgb = uint8x16x3_t(l, l, l);
        vst3q_u8(dst_ptr.add(dst_offset), rgb);
    }

    let src_rem = &src[simd_width * 16..];
    let dst_rem = &mut dst[simd_width * 48..];
    for i in 0..remainder {
        let l = src_rem[i];
        dst_rem[i * 3] = l;
        dst_rem[i * 3 + 1] = l;
        dst_rem[i * 3 + 2] = l;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_la_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 8;
    let remainder = width % 8;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 32;

        let la = vld2q_u8(src_ptr.add(src_offset));
        let l = la.0;
        let a = la.1;

        let rgba = uint8x16x4_t(l, l, l, a);
        vst4q_u8(dst_ptr.add(dst_offset), rgba);
    }

    let src_rem = &src[simd_width * 16..];
    let dst_rem = &mut dst[simd_width * 32..];
    for i in 0..remainder {
        let l = src_rem[i * 2];
        let a = src_rem[i * 2 + 1];
        dst_rem[i * 4] = l;
        dst_rem[i * 4 + 1] = l;
        dst_rem[i * 4 + 2] = l;
        dst_rem[i * 4 + 3] = a;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_rgba_to_la_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let r_w = vdupq_n_u16(54);
    let g_w = vdupq_n_u16(183);
    let b_w = vdupq_n_u16(19);

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 32;

        let rgba = vld4q_u8(src_ptr.add(src_offset));
        let r = rgba.0;
        let g = rgba.1;
        let b = rgba.2;
        let a = rgba.3;

        let r_lo = vmovl_u8(vget_low_u8(r));
        let r_hi = vmovl_u8(vget_high_u8(r));
        let g_lo = vmovl_u8(vget_low_u8(g));
        let g_hi = vmovl_u8(vget_high_u8(g));
        let b_lo = vmovl_u8(vget_low_u8(b));
        let b_hi = vmovl_u8(vget_high_u8(b));

        let sum_lo = vaddq_u16(
            vaddq_u16(vmulq_u16(r_lo, r_w), vmulq_u16(g_lo, g_w)),
            vmulq_u16(b_lo, b_w),
        );
        let sum_hi = vaddq_u16(
            vaddq_u16(vmulq_u16(r_hi, r_w), vmulq_u16(g_hi, g_w)),
            vmulq_u16(b_hi, b_w),
        );

        let lum_lo = vshrn_n_u16(sum_lo, 8);
        let lum_hi = vshrn_n_u16(sum_hi, 8);
        let lum = vcombine_u8(lum_lo, lum_hi);

        let la = uint8x16x2_t(lum, a);
        vst2q_u8(dst_ptr.add(dst_offset), la);
    }

    let src_rem = &src[simd_width * 64..];
    let dst_rem = &mut dst[simd_width * 32..];
    for i in 0..remainder {
        let r = src_rem[i * 4] as u32;
        let g = src_rem[i * 4 + 1] as u32;
        let b = src_rem[i * 4 + 2] as u32;
        let a = src_rem[i * 4 + 3];
        dst_rem[i * 2] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
        dst_rem[i * 2 + 1] = a;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_f32_to_u8_row_neon(src: &[f32], dst: &mut [u8]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = vdupq_n_f32(255.0);
    let zero = vdupq_n_f32(0.0);
    let max = vdupq_n_f32(255.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let f0 = vld1q_f32(src.as_ptr().add(src_offset));
        let f1 = vld1q_f32(src.as_ptr().add(src_offset + 4));
        let f2 = vld1q_f32(src.as_ptr().add(src_offset + 8));
        let f3 = vld1q_f32(src.as_ptr().add(src_offset + 12));

        let scaled0 = vminq_f32(vmaxq_f32(vmulq_f32(f0, scale), zero), max);
        let scaled1 = vminq_f32(vmaxq_f32(vmulq_f32(f1, scale), zero), max);
        let scaled2 = vminq_f32(vmaxq_f32(vmulq_f32(f2, scale), zero), max);
        let scaled3 = vminq_f32(vmaxq_f32(vmulq_f32(f3, scale), zero), max);

        let u0 = vcvtq_u32_f32(scaled0);
        let u1 = vcvtq_u32_f32(scaled1);
        let u2 = vcvtq_u32_f32(scaled2);
        let u3 = vcvtq_u32_f32(scaled3);

        let words_lo = vcombine_u16(vmovn_u32(u0), vmovn_u32(u1));
        let words_hi = vcombine_u16(vmovn_u32(u2), vmovn_u32(u3));

        let bytes = vcombine_u8(vmovn_u16(words_lo), vmovn_u16(words_hi));

        vst1q_u8(dst.as_mut_ptr().add(dst_offset), bytes);
    }

    for i in 0..remainder {
        let val = (src[simd_width * 16 + i] * 255.0).clamp(0.0, 255.0) as u8;
        dst[simd_width * 16 + i] = val;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_u8_to_u16_row_neon(src: &[u8], dst: &mut [u16]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let bytes = vld1q_u8(src.as_ptr().add(src_offset));

        let words_lo = vmovl_u8(vget_low_u8(bytes));
        let words_hi = vmovl_u8(vget_high_u8(bytes));

        let scaled_lo = vorrq_u16(words_lo, vshlq_n_u16(words_lo, 8));
        let scaled_hi = vorrq_u16(words_hi, vshlq_n_u16(words_hi, 8));

        vst1q_u16(dst.as_mut_ptr().add(dst_offset), scaled_lo);
        vst1q_u16(dst.as_mut_ptr().add(dst_offset + 8), scaled_hi);
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = (src[simd_width * 16 + i] as u16) * 257;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_u16_to_u8_row_neon(src: &[u16], dst: &mut [u8]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let words_lo = vld1q_u16(src.as_ptr().add(src_offset));
        let words_hi = vld1q_u16(src.as_ptr().add(src_offset + 8));

        let shifted_lo = vshrq_n_u16(words_lo, 8);
        let shifted_hi = vshrq_n_u16(words_hi, 8);

        let bytes = vcombine_u8(vmovn_u16(shifted_lo), vmovn_u16(shifted_hi));

        vst1q_u8(dst.as_mut_ptr().add(dst_offset), bytes);
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = (src[simd_width * 16 + i] / 257) as u8;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_u16_to_f32_row_neon(src: &[u16], dst: &mut [f32]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    let scale = vdupq_n_f32(1.0 / 65535.0);

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        let words = vld1q_u16(src.as_ptr().add(src_offset));

        let dwords_lo = vmovl_u16(vget_low_u16(words));
        let dwords_hi = vmovl_u16(vget_high_u16(words));

        let floats_lo = vmulq_f32(vcvtq_f32_u32(dwords_lo), scale);
        let floats_hi = vmulq_f32(vcvtq_f32_u32(dwords_hi), scale);

        vst1q_f32(dst.as_mut_ptr().add(dst_offset), floats_lo);
        vst1q_f32(dst.as_mut_ptr().add(dst_offset + 4), floats_hi);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = src[simd_width * 8 + i] as f32 / 65535.0;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) unsafe fn convert_f32_to_u16_row_neon(src: &[f32], dst: &mut [u16]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    let scale = vdupq_n_f32(65535.0);
    let zero = vdupq_n_f32(0.0);
    let max = vdupq_n_f32(65535.0);

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        let f0 = vld1q_f32(src.as_ptr().add(src_offset));
        let f1 = vld1q_f32(src.as_ptr().add(src_offset + 4));

        let scaled0 = vminq_f32(vmaxq_f32(vmulq_f32(f0, scale), zero), max);
        let scaled1 = vminq_f32(vmaxq_f32(vmulq_f32(f1, scale), zero), max);

        let u0 = vcvtq_u32_f32(scaled0);
        let u1 = vcvtq_u32_f32(scaled1);

        let words = vcombine_u16(vmovn_u32(u0), vmovn_u32(u1));

        vst1q_u16(dst.as_mut_ptr().add(dst_offset), words);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = (src[simd_width * 8 + i] * 65535.0).clamp(0.0, 65535.0) as u16;
    }
}
