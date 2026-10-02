//! NEON row conversion kernels for aarch64.

use std::arch::aarch64::{
    float32x4_t, float64x2_t, uint8x16_t, uint8x16x3_t, uint8x16x4_t, uint16x4_t, uint16x8_t,
    uint32x4_t, vaddq_u16, vaddq_u32, vandq_u32, vcombine_u8, vcombine_u16, vcombine_u32,
    vcvt_f64_f32, vcvt_high_f64_f32, vcvtnq_u64_f64, vcvtq_f32_u32, vdivq_f32, vdupq_n_f32,
    vdupq_n_f64, vdupq_n_u8, vdupq_n_u16, vdupq_n_u32, vget_high_u8, vget_high_u16, vget_low_f32,
    vget_low_u8, vget_low_u16, vld1q_f32, vld1q_u8, vld1q_u16, vld3q_u8, vld4q_u8, vminq_f64,
    vmlal_n_u16, vmovl_u8, vmovl_u16, vmovn_u16, vmovn_u32, vmovn_u64, vmull_high_u16, vmull_n_u16,
    vmulq_f64, vorrq_u16, vshlq_n_u16, vshrn_n_u32, vshrq_n_u16, vshrq_n_u32, vst1q_f32, vst1q_u8,
    vst1q_u16, vst3q_u8, vst4q_u8,
};

use crate::common::luma;
use crate::image::conversion::scalar;
use crate::image::conversion::simd::ElementRow;

/// Sixteen `RGBA_U8` pixels per iteration: one deinterleaving load, one interleaving store.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_rgba_to_rgb_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels * 4].as_chunks::<64>();
    let (out_groups, _) = dst[..pixels * 3].as_chunks_mut::<48>();

    for (group, out) in groups.iter().zip(out_groups) {
        // SAFETY: the load reads the group's 64 bytes and the store writes the 48 of `out`.
        unsafe {
            let rgba = vld4q_u8(group.as_ptr());
            vst3q_u8(out.as_mut_ptr(), uint8x16x3_t(rgba.0, rgba.1, rgba.2));
        }
    }

    scalar::convert_row::<u8, u8, 4, 3>(&src[pixels * 4..], &mut dst[pixels * 3..], width - pixels);
}

/// Sixteen `RGB_U8` pixels per iteration: one deinterleaving load, one interleaving store.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_rgb_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    let alpha = vdupq_n_u8(u8::MAX);
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels * 3].as_chunks::<48>();
    let (out_groups, _) = dst[..pixels * 4].as_chunks_mut::<64>();

    for (group, out) in groups.iter().zip(out_groups) {
        // SAFETY: the load reads the group's 48 bytes and the store writes the 64 of `out`.
        unsafe {
            let rgb = vld3q_u8(group.as_ptr());
            vst4q_u8(out.as_mut_ptr(), uint8x16x4_t(rgb.0, rgb.1, rgb.2, alpha));
        }
    }

    scalar::convert_row::<u8, u8, 3, 4>(&src[pixels * 3..], &mut dst[pixels * 4..], width - pixels);
}

/// One channel of sixteen pixels, split into the four `u16` quads the
/// accumulator works on.
#[inline]
#[target_feature(enable = "neon")]
fn quads(channel: uint8x16_t) -> [uint16x4_t; 4] {
    let lo = vmovl_u8(vget_low_u8(channel));
    let hi = vmovl_u8(vget_high_u8(channel));
    [
        vget_low_u16(lo),
        vget_high_u16(lo),
        vget_low_u16(hi),
        vget_high_u16(hi),
    ]
}

/// The Rec. 709 luminance of four pixels, rounded as [`luma::round_q16`] rounds.
///
/// The widening multiply-accumulate takes *unsigned* 16-bit scalars and lands in
/// `u32` lanes, so the full weights go in as they are and the sum is the scalar
/// reference's exactly — not the 8-bit approximation a `u16` accumulator would
/// force.
#[inline]
#[target_feature(enable = "neon")]
fn luminance(r: uint16x4_t, g: uint16x4_t, b: uint16x4_t) -> uint16x4_t {
    const _: () = assert!(luma::WEIGHTS_Q16[1] <= u16::MAX as u32);
    let [wr, wg, wb] = luma::WEIGHTS_Q16.map(|weight| weight as u16);
    let sum = vmlal_n_u16(vmlal_n_u16(vmull_n_u16(r, wr), g, wg), b, wb);
    let odd = vandq_u32(vshrq_n_u32::<16>(sum), vdupq_n_u32(1));
    vshrn_n_u32::<16>(vaddq_u32(vaddq_u32(sum, vdupq_n_u32(0x7FFF)), odd))
}

/// Four luminance quads — sixteen values — packed back into 16 bytes.
#[inline]
#[target_feature(enable = "neon")]
fn pack_quads(lum: [uint16x4_t; 4]) -> uint8x16_t {
    vcombine_u8(
        vmovn_u16(vcombine_u16(lum[0], lum[1])),
        vmovn_u16(vcombine_u16(lum[2], lum[3])),
    )
}

/// The luminance of sixteen pixels given as deinterleaved channels.
#[inline]
#[target_feature(enable = "neon")]
fn luminance16(r: uint8x16_t, g: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let (r, g, b) = (quads(r), quads(g), quads(b));
    pack_quads([0, 1, 2, 3].map(|quad| luminance(r[quad], g[quad], b[quad])))
}

/// Sixteen `RGBA_U8` pixels per iteration: one deinterleaving 64-byte load, one
/// 16-byte store.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_rgba_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels * 4].as_chunks::<64>();
    let (out_groups, _) = dst[..pixels].as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups) {
        // SAFETY: the load reads the group's 64 bytes and the store writes the 16 of `out`.
        unsafe {
            let rgba = vld4q_u8(group.as_ptr());
            vst1q_u8(out.as_mut_ptr(), luminance16(rgba.0, rgba.1, rgba.2));
        }
    }

    scalar::convert_row::<u8, u8, 4, 1>(&src[pixels * 4..], &mut dst[pixels..], width - pixels);
}

/// Sixteen `RGB_U8` pixels per iteration: one deinterleaving 48-byte load, one
/// 16-byte store.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_rgb_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels * 3].as_chunks::<48>();
    let (out_groups, _) = dst[..pixels].as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups) {
        // SAFETY: the load reads the group's 48 bytes and the store writes the 16 of `out`.
        unsafe {
            let rgb = vld3q_u8(group.as_ptr());
            vst1q_u8(out.as_mut_ptr(), luminance16(rgb.0, rgb.1, rgb.2));
        }
    }

    scalar::convert_row::<u8, u8, 3, 1>(&src[pixels * 3..], &mut dst[pixels..], width - pixels);
}

/// Sixteen `L_U8` pixels per iteration: one load, one interleaving store.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_l_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    let alpha = vdupq_n_u8(u8::MAX);
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels].as_chunks::<16>();
    let (out_groups, _) = dst[..pixels * 4].as_chunks_mut::<64>();

    for (group, out) in groups.iter().zip(out_groups) {
        // SAFETY: the load reads the group's 16 bytes and the store writes the 64 of `out`.
        unsafe {
            let grey = vld1q_u8(group.as_ptr());
            vst4q_u8(out.as_mut_ptr(), uint8x16x4_t(grey, grey, grey, alpha));
        }
    }

    scalar::convert_row::<u8, u8, 1, 4>(&src[pixels..], &mut dst[pixels * 4..], width - pixels);
}

/// Sixteen `L_U8` pixels per iteration: one load, one interleaving store.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_l_to_rgb_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels].as_chunks::<16>();
    let (out_groups, _) = dst[..pixels * 3].as_chunks_mut::<48>();

    for (group, out) in groups.iter().zip(out_groups) {
        // SAFETY: the load reads the group's 16 bytes and the store writes the 48 of `out`.
        unsafe {
            let grey = vld1q_u8(group.as_ptr());
            vst3q_u8(out.as_mut_ptr(), uint8x16x3_t(grey, grey, grey));
        }
    }

    scalar::convert_row::<u8, u8, 1, 3>(&src[pixels..], &mut dst[pixels * 3..], width - pixels);
}

/// Four `f32` on `[0, 1]` scaled by `full_scale` and rounded as [`Sample::from_unit`] does:
/// the product in `f64`, clamped above, and converted by `fcvtnu`, which rounds to nearest with
/// ties to even, saturates below at zero and takes NaN to zero.
///
/// [`Sample::from_unit`]: crate::common::sample::Sample::from_unit
#[inline]
#[target_feature(enable = "neon")]
fn narrow4(values: float32x4_t, full_scale: float64x2_t) -> uint32x4_t {
    let narrow = |half: float64x2_t| {
        vmovn_u64(vcvtnq_u64_f64(vminq_f64(
            vmulq_f64(half, full_scale),
            full_scale,
        )))
    };
    vcombine_u32(
        narrow(vcvt_f64_f32(vget_low_f32(values))),
        narrow(vcvt_high_f64_f32(values)),
    )
}

/// Sixteen samples per iteration.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_f32_to_u8_row_neon(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<f32, u8>::new(src, dst);
    let full_scale = vdupq_n_f64(255.0);
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let (quads, _) = group.as_chunks::<4>();
        // SAFETY: each load reads one four-float quad of the group.
        let ints = [0, 1, 2, 3]
            .map(|quad| narrow4(unsafe { vld1q_f32(quads[quad].as_ptr()) }, full_scale));
        let words = [
            vcombine_u16(vmovn_u32(ints[0]), vmovn_u32(ints[1])),
            vcombine_u16(vmovn_u32(ints[2]), vmovn_u32(ints[3])),
        ];
        // SAFETY: the store writes the 16 bytes of `out`.
        unsafe {
            vst1q_u8(
                out.as_mut_ptr(),
                vcombine_u8(vmovn_u16(words[0]), vmovn_u16(words[1])),
            );
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Sixteen samples per iteration.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_u8_to_f32_row_neon(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u8, f32>::new(src, dst);
    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = vdupq_n_f32(255.0);
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        // SAFETY: the load reads the group's 16 bytes.
        let bytes = unsafe { vld1q_u8(group.as_ptr()) };
        let words = [vmovl_u8(vget_low_u8(bytes)), vmovl_u8(vget_high_u8(bytes))];
        let dwords = [
            vmovl_u16(vget_low_u16(words[0])),
            vmovl_u16(vget_high_u16(words[0])),
            vmovl_u16(vget_low_u16(words[1])),
            vmovl_u16(vget_high_u16(words[1])),
        ];
        let (out, _) = out.as_chunks_mut::<4>();
        for (out, dwords) in out.iter_mut().zip(dwords) {
            // SAFETY: the store writes the four floats of `out`.
            unsafe { vst1q_f32(out.as_mut_ptr(), vdivq_f32(vcvtq_f32_u32(dwords), divisor)) };
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Sixteen samples per iteration: `v · 257` is `v` in both bytes.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_u8_to_u16_row_neon(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u8, u16>::new(src, dst);
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        // SAFETY: the load reads the group's 16 bytes.
        let bytes = unsafe { vld1q_u8(group.as_ptr()) };
        let words = [vmovl_u8(vget_low_u8(bytes)), vmovl_u8(vget_high_u8(bytes))];
        let (out, _) = out.as_chunks_mut::<8>();
        for (out, words) in out.iter_mut().zip(words) {
            // SAFETY: the store writes the eight words of `out`.
            unsafe { vst1q_u16(out.as_mut_ptr(), vorrq_u16(words, vshlq_n_u16::<8>(words))) };
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// `round(v / 257)` for eight `u16` lanes, exactly: `⌊v · 65281 / 2¹⁶⌋` stays within 65280, so
/// adding 128 cannot overflow, and the shift by 8 then gives the nearest quotient for every `v`
/// — `v / 257` is never a tie, 257 being odd.
#[inline]
#[target_feature(enable = "neon")]
fn divide_by_257(words: uint16x8_t) -> uint16x8_t {
    let quotient = vcombine_u16(
        vshrn_n_u32::<16>(vmull_n_u16(vget_low_u16(words), 65281)),
        vshrn_n_u32::<16>(vmull_high_u16(words, vdupq_n_u16(65281))),
    );
    vshrq_n_u16::<8>(vaddq_u16(quotient, vdupq_n_u16(128)))
}

/// Sixteen samples per iteration.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_u16_to_u8_row_neon(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u16, u8>::new(src, dst);
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let (halves, _) = group.as_chunks::<8>();
        // SAFETY: each load reads one half of the group's sixteen words.
        let [lo, hi] =
            [0, 1].map(|half| divide_by_257(unsafe { vld1q_u16(halves[half].as_ptr()) }));
        // SAFETY: the store writes the 16 bytes of `out`.
        unsafe { vst1q_u8(out.as_mut_ptr(), vcombine_u8(vmovn_u16(lo), vmovn_u16(hi))) };
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Eight samples per iteration.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_u16_to_f32_row_neon(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u16, f32>::new(src, dst);
    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = vdupq_n_f32(65535.0);
    let (groups, src_tail) = src.as_chunks::<8>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<8>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        // SAFETY: the load reads the group's eight words.
        let words = unsafe { vld1q_u16(group.as_ptr()) };
        let dwords = [
            vmovl_u16(vget_low_u16(words)),
            vmovl_u16(vget_high_u16(words)),
        ];
        let (out, _) = out.as_chunks_mut::<4>();
        for (out, dwords) in out.iter_mut().zip(dwords) {
            // SAFETY: the store writes the four floats of `out`.
            unsafe { vst1q_f32(out.as_mut_ptr(), vdivq_f32(vcvtq_f32_u32(dwords), divisor)) };
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Eight samples per iteration.
#[target_feature(enable = "neon")]
pub(super) unsafe fn convert_f32_to_u16_row_neon(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<f32, u16>::new(src, dst);
    let full_scale = vdupq_n_f64(65535.0);
    let (groups, src_tail) = src.as_chunks::<8>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<8>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let (quads, _) = group.as_chunks::<4>();
        // SAFETY: each load reads one four-float quad of the group.
        let [lo, hi] =
            [0, 1].map(|quad| narrow4(unsafe { vld1q_f32(quads[quad].as_ptr()) }, full_scale));
        // SAFETY: the store writes the eight words of `out`.
        unsafe { vst1q_u16(out.as_mut_ptr(), vcombine_u16(vmovn_u32(lo), vmovn_u32(hi))) };
    }

    scalar::convert_elements(src_tail, dst_tail);
}
