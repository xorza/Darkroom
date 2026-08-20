//! NEON row kernels. Each applies [`ChannelAffine`] to a row in place and
//! finishes the sub-vector tail with the scalar reference, so the vector body
//! and the tail produce identical results.
//!
//! The multiply and add are kept separate rather than fused: `vfma` would round
//! once where the scalar reference rounds twice, and the integer formats are
//! cross-checked for bit equality.

use std::arch::aarch64::*;

use crate::ops::contrast_brightness::cpu::{ChannelAffine, ContrastBrightnessApply};

/// The affine's constants, splatted once per row.
#[derive(Debug, Clone, Copy)]
struct Splat {
    scale: float32x4_t,
    offset: float32x4_t,
    min: float32x4_t,
    max: float32x4_t,
}

impl Splat {
    fn new(affine: ChannelAffine) -> Self {
        // SAFETY: NEON is baseline on aarch64.
        unsafe {
            Self {
                scale: vdupq_n_f32(affine.scale),
                offset: vdupq_n_f32(affine.offset),
                min: vdupq_n_f32(0.0),
                max: vdupq_n_f32(affine.max),
            }
        }
    }

    /// Four unsigned integer channel values, widened to `u32` lanes, in and out.
    #[inline]
    unsafe fn apply_u32(self, values: uint32x4_t) -> uint32x4_t {
        unsafe {
            let scaled = vaddq_f32(vmulq_f32(vcvtq_f32_u32(values), self.scale), self.offset);
            // `vcvtnq` rounds to nearest, ties to even, matching the scalar
            // reference's `round_ties_even`.
            vcvtnq_u32_f32(vminq_f32(vmaxq_f32(scaled, self.min), self.max))
        }
    }

    /// Four `f32` channel values in and out.
    #[inline]
    unsafe fn apply_f32(self, values: float32x4_t) -> float32x4_t {
        unsafe {
            let scaled = vaddq_f32(vmulq_f32(values, self.scale), self.offset);
            vminq_f32(vmaxq_f32(scaled, self.min), self.max)
        }
    }

    #[inline]
    unsafe fn apply_u16x8(self, values: uint16x8_t) -> uint16x8_t {
        unsafe {
            let lo = self.apply_u32(vmovl_u16(vget_low_u16(values)));
            let hi = self.apply_u32(vmovl_high_u16(values));
            vcombine_u16(vmovn_u32(lo), vmovn_u32(hi))
        }
    }

    #[inline]
    unsafe fn apply_u8x16(self, values: uint8x16_t) -> uint8x16_t {
        unsafe {
            let lo = self.apply_u16x8(vmovl_u8(vget_low_u8(values)));
            let hi = self.apply_u16x8(vmovl_high_u8(values));
            vcombine_u8(vmovn_u16(lo), vmovn_u16(hi))
        }
    }
}

/// 16 `u8` channel values per iteration.
pub(super) unsafe fn u8_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row = &mut row[..count];

    let (chunks, tail) = row.as_chunks_mut::<16>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 16 bytes, the width of one load and store.
        unsafe {
            let values = vld1q_u8(chunk.as_ptr());
            vst1q_u8(chunk.as_mut_ptr(), splat.apply_u8x16(values));
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// 16 `RGBA_U8` pixels per iteration, alpha carried through untouched.
pub(super) unsafe fn u8_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row = &mut row[..pixels * 4];

    let (chunks, tail) = row.as_chunks_mut::<64>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 64 bytes — 16 RGBA pixels, one `vld4q`.
        unsafe {
            let pixels = vld4q_u8(chunk.as_ptr());
            let out = uint8x16x4_t(
                splat.apply_u8x16(pixels.0),
                splat.apply_u8x16(pixels.1),
                splat.apply_u8x16(pixels.2),
                pixels.3,
            );
            vst4q_u8(chunk.as_mut_ptr(), out);
        }
    }

    for pixel in tail.as_chunks_mut::<4>().0 {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}

/// Eight `u16` channel values per iteration.
pub(super) unsafe fn u16_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [u16] = bytemuck::cast_slice_mut(&mut row[..count * 2]);

    let (chunks, tail) = row.as_chunks_mut::<8>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly eight `u16` — one load and store.
        unsafe {
            let values = vld1q_u16(chunk.as_ptr());
            vst1q_u16(chunk.as_mut_ptr(), splat.apply_u16x8(values));
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// Eight `RGBA_U16` pixels per iteration, alpha carried through untouched.
pub(super) unsafe fn u16_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [u16] = bytemuck::cast_slice_mut(&mut row[..pixels * 8]);

    let (chunks, tail) = row.as_chunks_mut::<32>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 32 `u16` — eight RGBA pixels, one `vld4q`.
        unsafe {
            let pixels = vld4q_u16(chunk.as_ptr());
            let out = uint16x8x4_t(
                splat.apply_u16x8(pixels.0),
                splat.apply_u16x8(pixels.1),
                splat.apply_u16x8(pixels.2),
                pixels.3,
            );
            vst4q_u16(chunk.as_mut_ptr(), out);
        }
    }

    for pixel in tail.as_chunks_mut::<4>().0 {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}

/// Four `f32` channel values per iteration.
pub(super) unsafe fn f32_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [f32] = bytemuck::cast_slice_mut(&mut row[..count * 4]);

    let (chunks, tail) = row.as_chunks_mut::<4>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly four `f32` — one load and store.
        unsafe {
            let values = vld1q_f32(chunk.as_ptr());
            vst1q_f32(chunk.as_mut_ptr(), splat.apply_f32(values));
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// Four `RGBA_F32` pixels per iteration, alpha carried through untouched.
pub(super) unsafe fn f32_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [f32] = bytemuck::cast_slice_mut(&mut row[..pixels * 16]);

    let (chunks, tail) = row.as_chunks_mut::<16>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 16 `f32` — four RGBA pixels, one `vld4q`.
        unsafe {
            let pixels = vld4q_f32(chunk.as_ptr());
            let out = float32x4x4_t(
                splat.apply_f32(pixels.0),
                splat.apply_f32(pixels.1),
                splat.apply_f32(pixels.2),
                pixels.3,
            );
            vst4q_f32(chunk.as_mut_ptr(), out);
        }
    }

    for pixel in tail.as_chunks_mut::<4>().0 {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}
