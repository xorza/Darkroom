//! NEON row conversion implementations for aarch64.

#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::aarch64::*;

use crate::image::conversion::simd;
use crate::image::conversion::{LUMA_B, LUMA_G, LUMA_R};


pub(super) unsafe fn convert_rgba_to_rgb_row_neon(src: &[u8], dst: &mut [u8], width: usize) {

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

pub(super) unsafe fn convert_rgb_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {

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

/// One channel of sixteen pixels, split into the four `u16` quads the
/// accumulator works on.
#[inline]
unsafe fn quads(channel: uint8x16_t) -> [uint16x4_t; 4] {
    unsafe {
        let lo = vmovl_u8(vget_low_u8(channel));
        let hi = vmovl_u8(vget_high_u8(channel));
        [
            vget_low_u16(lo),
            vget_high_u16(lo),
            vget_low_u16(hi),
            vget_high_u16(hi),
        ]
    }
}

/// The Rec. 709 luminance of four pixels.
///
/// The widening multiply-accumulate takes *unsigned* 16-bit scalars and lands in
/// `u32` lanes, so the full weights go in as they are and the sum is the scalar
/// reference's exactly — not the 8-bit approximation a `u16` accumulator would
/// force. `vshrn` folds the reference's `>> 16` into the narrowing store.
#[inline]
unsafe fn luminance(r: uint16x4_t, g: uint16x4_t, b: uint16x4_t) -> uint16x4_t {
    unsafe {
        let sum = vmull_n_u16(r, LUMA_R as u16);
        let sum = vmlal_n_u16(sum, g, LUMA_G as u16);
        let sum = vmlal_n_u16(sum, b, LUMA_B as u16);
        vshrn_n_u32::<16>(sum)
    }
}

/// Four luminance quads — sixteen values — packed back into 16 bytes.
#[inline]
unsafe fn pack_quads(lum: [uint16x4_t; 4]) -> uint8x16_t {
    unsafe {
        vcombine_u8(
            vmovn_u16(vcombine_u16(lum[0], lum[1])),
            vmovn_u16(vcombine_u16(lum[2], lum[3])),
        )
    }
}

/// Sixteen `RGBA_U8` pixels per iteration: one deinterleaving 64-byte load, one
/// 16-byte store.
pub(super) unsafe fn convert_rgba_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    // `src` runs to the end of the image; only this row's pixels are ours.
    let src = &src[..width * 4];
    let (groups, src_tail) = src.as_chunks::<64>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups) {
        // SAFETY: a group is exactly 64 bytes — one `vld4q_u8` — and the store
        // fills its whole 16-byte chunk.
        unsafe {
            let rgba = vld4q_u8(group.as_ptr());
            let (r, g, b) = (quads(rgba.0), quads(rgba.1), quads(rgba.2));
            let lum = [
                luminance(r[0], g[0], b[0]),
                luminance(r[1], g[1], b[1]),
                luminance(r[2], g[2], b[2]),
                luminance(r[3], g[3], b[3]),
            ];
            vst1q_u8(out.as_mut_ptr(), pack_quads(lum));
        }
    }

    simd::luma_tail::<4>(src_tail, dst_tail);
}

/// Sixteen `RGB_U8` pixels per iteration: one deinterleaving 48-byte load, one
/// 16-byte store.
pub(super) unsafe fn convert_rgb_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    // `src` runs to the end of the image; only this row's pixels are ours.
    let src = &src[..width * 3];
    let (groups, src_tail) = src.as_chunks::<48>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups) {
        // SAFETY: a group is exactly 48 bytes — one `vld3q_u8` — and the store
        // fills its whole 16-byte chunk.
        unsafe {
            let rgb = vld3q_u8(group.as_ptr());
            let (r, g, b) = (quads(rgb.0), quads(rgb.1), quads(rgb.2));
            let lum = [
                luminance(r[0], g[0], b[0]),
                luminance(r[1], g[1], b[1]),
                luminance(r[2], g[2], b[2]),
                luminance(r[3], g[3], b[3]),
            ];
            vst1q_u8(out.as_mut_ptr(), pack_quads(lum));
        }
    }

    simd::luma_tail::<3>(src_tail, dst_tail);
}

pub(super) unsafe fn convert_l_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {

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

pub(super) unsafe fn convert_l_to_rgb_row_neon(src: &[u8], dst: &mut [u8], width: usize) {

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

pub(super) unsafe fn convert_f32_to_u8_row_neon(src: &[f32], dst: &mut [u8]) {

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

        let u0 = vcvtnq_u32_f32(scaled0);
        let u1 = vcvtnq_u32_f32(scaled1);
        let u2 = vcvtnq_u32_f32(scaled2);
        let u3 = vcvtnq_u32_f32(scaled3);

        let words_lo = vcombine_u16(vmovn_u32(u0), vmovn_u32(u1));
        let words_hi = vcombine_u16(vmovn_u32(u2), vmovn_u32(u3));

        let bytes = vcombine_u8(vmovn_u16(words_lo), vmovn_u16(words_hi));

        vst1q_u8(dst.as_mut_ptr().add(dst_offset), bytes);
    }

    for i in 0..remainder {
        let val = (src[simd_width * 16 + i] * 255.0).round_ties_even().clamp(0.0, 255.0) as u8;
        dst[simd_width * 16 + i] = val;
    }
}

pub(super) unsafe fn convert_u8_to_f32_row_neon(src: &[u8], dst: &mut [f32]) {

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = vdupq_n_f32(255.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let bytes = vld1q_u8(src.as_ptr().add(src_offset));

        // Unpack bytes to 16-bit words
        let words_lo = vmovl_u8(vget_low_u8(bytes));
        let words_hi = vmovl_u8(vget_high_u8(bytes));

        // Unpack 16-bit words to 32-bit dwords and convert to float
        let dwords_0 = vmovl_u16(vget_low_u16(words_lo));
        let dwords_1 = vmovl_u16(vget_high_u16(words_lo));
        let dwords_2 = vmovl_u16(vget_low_u16(words_hi));
        let dwords_3 = vmovl_u16(vget_high_u16(words_hi));

        let floats_0 = vdivq_f32(vcvtq_f32_u32(dwords_0), divisor);
        let floats_1 = vdivq_f32(vcvtq_f32_u32(dwords_1), divisor);
        let floats_2 = vdivq_f32(vcvtq_f32_u32(dwords_2), divisor);
        let floats_3 = vdivq_f32(vcvtq_f32_u32(dwords_3), divisor);

        vst1q_f32(dst.as_mut_ptr().add(dst_offset), floats_0);
        vst1q_f32(dst.as_mut_ptr().add(dst_offset + 4), floats_1);
        vst1q_f32(dst.as_mut_ptr().add(dst_offset + 8), floats_2);
        vst1q_f32(dst.as_mut_ptr().add(dst_offset + 12), floats_3);
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = src[simd_width * 16 + i] as f32 / 255.0;
    }
}

pub(super) unsafe fn convert_u8_to_u16_row_neon(src: &[u8], dst: &mut [u16]) {

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

pub(super) unsafe fn convert_u16_to_u8_row_neon(src: &[u16], dst: &mut [u8]) {

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

pub(super) unsafe fn convert_u16_to_f32_row_neon(src: &[u16], dst: &mut [f32]) {

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = vdupq_n_f32(65535.0);

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        let words = vld1q_u16(src.as_ptr().add(src_offset));

        let dwords_lo = vmovl_u16(vget_low_u16(words));
        let dwords_hi = vmovl_u16(vget_high_u16(words));

        let floats_lo = vdivq_f32(vcvtq_f32_u32(dwords_lo), divisor);
        let floats_hi = vdivq_f32(vcvtq_f32_u32(dwords_hi), divisor);

        vst1q_f32(dst.as_mut_ptr().add(dst_offset), floats_lo);
        vst1q_f32(dst.as_mut_ptr().add(dst_offset + 4), floats_hi);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = src[simd_width * 8 + i] as f32 / 65535.0;
    }
}

pub(super) unsafe fn convert_f32_to_u16_row_neon(src: &[f32], dst: &mut [u16]) {

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

        let u0 = vcvtnq_u32_f32(scaled0);
        let u1 = vcvtnq_u32_f32(scaled1);

        let words = vcombine_u16(vmovn_u32(u0), vmovn_u32(u1));

        vst1q_u16(dst.as_mut_ptr().add(dst_offset), words);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = (src[simd_width * 8 + i] * 65535.0).round_ties_even().clamp(0.0, 65535.0) as u16;
    }
}
