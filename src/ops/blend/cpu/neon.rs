//! NEON blend kernels for `RGBA_U8` and `RGBA_F32`.
//!
//! One pixel's four channels fill one register, so the blend is a single
//! expression both storage types share — only the load and the store differ.
//!
//! Every step matches the scalar reference operation for operation, so the
//! vector results are bit-identical to it rather than within a rounding
//! tolerance: the byte path *divides* by 255 where multiplying by the reciprocal
//! would be faster, and the alpha mix is a separate multiply and add — `vfma`
//! would round once where the reference rounds twice. Each kernel finishes its
//! sub-vector tail with the reference itself.

use std::arch::aarch64::*;

use crate::ops::blend::cpu;
use crate::ops::blend::{Blend, BlendMode};

/// The blend's constants, splatted once per row. `max` is the storage type's
/// full-scale value: 255 for bytes, one for floats.
#[derive(Debug, Clone, Copy)]
struct Splat {
    alpha: float32x4_t,
    one_minus_alpha: float32x4_t,
    zero: float32x4_t,
    one: float32x4_t,
    half: float32x4_t,
    two: float32x4_t,
    max: float32x4_t,
    /// Set in lane 3 only — selects the alpha channel out of a blended pixel.
    alpha_lane: uint32x4_t,
}

impl Splat {
    fn new(alpha: f32, max: f32) -> Self {
        // SAFETY: NEON is baseline on aarch64.
        unsafe {
            Self {
                alpha: vdupq_n_f32(alpha),
                one_minus_alpha: vdupq_n_f32(1.0 - alpha),
                zero: vdupq_n_f32(0.0),
                one: vdupq_n_f32(1.0),
                half: vdupq_n_f32(0.5),
                two: vdupq_n_f32(2.0),
                max: vdupq_n_f32(max),
                alpha_lane: vsetq_lane_u32::<3>(u32::MAX, vdupq_n_u32(0)),
            }
        }
    }

    /// One pixel's four normalized channels, blended and mixed — the vector form
    /// of [`BlendMode::blend`].
    ///
    /// Lane 3 is alpha, which carries no mode of its own. `Normal`'s blended
    /// value is `src`, so restoring src in that lane is the whole difference.
    #[inline]
    unsafe fn blend(self, mode: BlendMode, src: float32x4_t, dst: float32x4_t) -> float32x4_t {
        unsafe {
            let blended = match mode {
                BlendMode::Normal => src,
                BlendMode::Add => vminq_f32(vaddq_f32(src, dst), self.one),
                BlendMode::Subtract => vmaxq_f32(vsubq_f32(dst, src), self.zero),
                BlendMode::Multiply => vmulq_f32(src, dst),
                BlendMode::Screen => vsubq_f32(
                    self.one,
                    vmulq_f32(vsubq_f32(self.one, src), vsubq_f32(self.one, dst)),
                ),
                BlendMode::Overlay => {
                    let dark = vmulq_f32(self.two, vmulq_f32(src, dst));
                    let light = vsubq_f32(
                        self.one,
                        vmulq_f32(
                            self.two,
                            vmulq_f32(vsubq_f32(self.one, src), vsubq_f32(self.one, dst)),
                        ),
                    );
                    vbslq_f32(vcltq_f32(dst, self.half), dark, light)
                }
            };
            let blended = vbslq_f32(self.alpha_lane, src, blended);
            vaddq_f32(
                vmulq_f32(blended, self.alpha),
                vmulq_f32(dst, self.one_minus_alpha),
            )
        }
    }

    /// Scales a blended value back into the storage type's range and clamps it
    /// there, in the reference's order: multiply, then clamp the product. For
    /// float storage `max` is one, so the multiply is exact and the clamp is the
    /// whole step.
    #[inline]
    unsafe fn scale_clamp(self, blended: float32x4_t) -> float32x4_t {
        unsafe { vminq_f32(vmaxq_f32(vmulq_f32(blended, self.max), self.zero), self.max) }
    }

    /// The four `RGBA_U8` pixels of one 16-byte load, widened and divided into
    /// `[0, 1]`.
    ///
    /// The divide is what the reference does. `value * (1.0 / 255.0)` would be
    /// several times cheaper but disagrees with it for 126 of the 256 byte
    /// values, which is enough to shift an output byte by one once the result is
    /// scaled back up and truncated.
    #[inline]
    unsafe fn normalize(self, bytes: uint8x16_t) -> [float32x4_t; 4] {
        unsafe {
            let lo = vmovl_u8(vget_low_u8(bytes));
            let hi = vmovl_u8(vget_high_u8(bytes));
            [
                self.divide(vmovl_u16(vget_low_u16(lo))),
                self.divide(vmovl_u16(vget_high_u16(lo))),
                self.divide(vmovl_u16(vget_low_u16(hi))),
                self.divide(vmovl_u16(vget_high_u16(hi))),
            ]
        }
    }

    #[inline]
    unsafe fn divide(self, values: uint32x4_t) -> float32x4_t {
        unsafe { vdivq_f32(vcvtq_f32_u32(values), self.max) }
    }
}

/// Four blended pixels, already clamped to `[0, 255]`, packed back into 16
/// bytes. `vcvtq_u32_f32` truncates toward zero, which is what the reference's
/// `as u8` does.
#[inline]
unsafe fn narrow(pixels: [float32x4_t; 4]) -> uint8x16_t {
    unsafe {
        let lo = vcombine_u16(
            vmovn_u32(vcvtq_u32_f32(pixels[0])),
            vmovn_u32(vcvtq_u32_f32(pixels[1])),
        );
        let hi = vcombine_u16(
            vmovn_u32(vcvtq_u32_f32(pixels[2])),
            vmovn_u32(vcvtq_u32_f32(pixels[3])),
        );
        vcombine_u8(vmovn_u16(lo), vmovn_u16(hi))
    }
}

/// Four `RGBA_U8` pixels per iteration: one 16-byte load from each row, four
/// pixel registers, one 16-byte store.
pub(super) unsafe fn rgba_u8_row(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    params: Blend,
) {
    let splat = Splat::new(params.alpha, f32::from(u8::MAX));
    let mode = params.mode;

    let (src_quads, src_tail) = src_row.as_chunks::<16>();
    let (dst_quads, dst_tail) = dst_row.as_chunks::<16>();
    let (out_quads, out_tail) = out_row.as_chunks_mut::<16>();

    for ((src, dst), out) in src_quads.iter().zip(dst_quads).zip(out_quads) {
        // SAFETY: every chunk is exactly 16 bytes — four RGBA pixels, the width
        // of one load and one store.
        unsafe {
            let src = splat.normalize(vld1q_u8(src.as_ptr()));
            let dst = splat.normalize(vld1q_u8(dst.as_ptr()));
            let blended = [
                splat.scale_clamp(splat.blend(mode, src[0], dst[0])),
                splat.scale_clamp(splat.blend(mode, src[1], dst[1])),
                splat.scale_clamp(splat.blend(mode, src[2], dst[2])),
                splat.scale_clamp(splat.blend(mode, src[3], dst[3])),
            ];
            vst1q_u8(out.as_mut_ptr(), narrow(blended));
        }
    }

    cpu::rgba_tail(params, src_tail, dst_tail, out_tail);
}

/// One `RGBA_F32` pixel per iteration — four floats already fill a register, so
/// there is nothing to widen and no tail to finish.
pub(super) unsafe fn rgba_f32_row(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    params: Blend,
) {
    let splat = Splat::new(params.alpha, 1.0);
    let mode = params.mode;

    let src_row: &[f32] = bytemuck::cast_slice(src_row);
    let dst_row: &[f32] = bytemuck::cast_slice(dst_row);
    let out_row: &mut [f32] = bytemuck::cast_slice_mut(out_row);

    let (src_pixels, rest) = src_row.as_chunks::<4>();
    let (dst_pixels, _) = dst_row.as_chunks::<4>();
    let (out_pixels, _) = out_row.as_chunks_mut::<4>();
    debug_assert!(rest.is_empty(), "an RGBA row is a whole number of pixels");

    for ((src, dst), out) in src_pixels.iter().zip(dst_pixels).zip(out_pixels) {
        // SAFETY: every chunk is exactly four `f32` — one RGBA pixel, the width
        // of one load and one store.
        unsafe {
            let src = vld1q_f32(src.as_ptr());
            let dst = vld1q_f32(dst.as_ptr());
            vst1q_f32(
                out.as_mut_ptr(),
                splat.scale_clamp(splat.blend(mode, src, dst)),
            );
        }
    }
}
