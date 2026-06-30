//! NEON specialization of the 4-channel (RGBA) transform path: one pixel is a
//! single `f32x4` lane, so the load → interpolate → store chain runs as vector
//! ops. L/RGB stay on the scalar path in the parent module.
//!
//! Every step is chosen to match the scalar reference bit-for-bit: widening is
//! exact for `0..=255` / `0..=65535`, `mix` uses two muls + an add (no FMA) so
//! per-lane rounding equals scalar, the clamp is `vmax`/`vmin`, and the
//! float→int convert truncates toward zero like `as`. The `neon_matches_scalar`
//! test asserts exact equality.

use std::arch::aarch64::*;

use glam::Vec2;
use rayon::prelude::*;

use super::{Transform, TransformElem};
use crate::image::Image;

/// A 4-channel element type with a NEON load/store of one pixel as an `f32x4`.
pub(super) trait NeonRgba: TransformElem {
    /// Loads the 4 channels at element index `base` as an `f32x4`.
    ///
    /// # Safety
    /// `base + 4 <= pixels.len()`.
    unsafe fn load4(pixels: &[Self], base: usize) -> float32x4_t;

    /// Clamps/converts `v` and stores it into the 4 channels at index `base`.
    ///
    /// # Safety
    /// `base + 4 <= out.len()`.
    unsafe fn store4(out: &mut [Self], base: usize, v: float32x4_t);
}

impl NeonRgba for u8 {
    #[inline]
    unsafe fn load4(pixels: &[u8], base: usize) -> float32x4_t {
        unsafe {
            let packed = (pixels.as_ptr().add(base) as *const u32).read_unaligned();
            let bytes = vcreate_u8(packed as u64); // low 4 lanes = this pixel
            let u16x = vmovl_u8(bytes);
            let u32x = vmovl_u16(vget_low_u16(u16x));
            vcvtq_f32_u32(u32x)
        }
    }
    #[inline]
    unsafe fn store4(out: &mut [u8], base: usize, v: float32x4_t) {
        unsafe {
            let clamped = vminq_f32(vmaxq_f32(v, vdupq_n_f32(0.0)), vdupq_n_f32(255.0));
            let u32x = vcvtq_u32_f32(clamped); // truncates toward zero
            let u8x = vmovn_u16(vcombine_u16(vmovn_u32(u32x), vmovn_u32(u32x)));
            let packed = vget_lane_u32::<0>(vreinterpret_u32_u8(u8x));
            (out.as_mut_ptr().add(base) as *mut u32).write_unaligned(packed);
        }
    }
}

impl NeonRgba for u16 {
    #[inline]
    unsafe fn load4(pixels: &[u16], base: usize) -> float32x4_t {
        unsafe { vcvtq_f32_u32(vmovl_u16(vld1_u16(pixels.as_ptr().add(base)))) }
    }
    #[inline]
    unsafe fn store4(out: &mut [u16], base: usize, v: float32x4_t) {
        unsafe {
            let clamped = vminq_f32(vmaxq_f32(v, vdupq_n_f32(0.0)), vdupq_n_f32(65535.0));
            vst1_u16(
                out.as_mut_ptr().add(base),
                vmovn_u32(vcvtq_u32_f32(clamped)),
            );
        }
    }
}

impl NeonRgba for f32 {
    #[inline]
    unsafe fn load4(pixels: &[f32], base: usize) -> float32x4_t {
        unsafe { vld1q_f32(pixels.as_ptr().add(base)) }
    }
    #[inline]
    unsafe fn store4(out: &mut [f32], base: usize, v: float32x4_t) {
        // Unclamped, matching the scalar f32 `from_f32`.
        unsafe { vst1q_f32(out.as_mut_ptr().add(base), v) }
    }
}

/// `mix(a, b, t) = a*(1-t) + b*t` per lane — two muls + add (no FMA), so the
/// rounding matches the scalar `mix`.
#[inline]
fn mix(a: float32x4_t, b: float32x4_t, t: f32) -> float32x4_t {
    unsafe { vaddq_f32(vmulq_n_f32(a, 1.0 - t), vmulq_n_f32(b, t)) }
}

#[inline]
fn read_tap<T: NeonRgba>(pixels: &[T], w: usize, h: usize, x: i32, y: i32) -> float32x4_t {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return unsafe { vdupq_n_f32(0.0) };
    }
    let base = (y as usize * w + x as usize) * 4;
    // SAFETY: the bounds check above guarantees `base + 4 <= pixels.len()`.
    unsafe { T::load4(pixels, base) }
}

#[inline]
fn sample_bilinear<T: NeonRgba>(pixels: &[T], w: usize, h: usize, pos: Vec2) -> float32x4_t {
    let fx0 = pos.x.floor();
    let fy0 = pos.y.floor();
    let fx = pos.x - fx0;
    let fy = pos.y - fy0;
    let x0 = fx0 as i32;
    let y0 = fy0 as i32;

    let c00 = read_tap(pixels, w, h, x0, y0);
    let c10 = read_tap(pixels, w, h, x0 + 1, y0);
    let c01 = read_tap(pixels, w, h, x0, y0 + 1);
    let c11 = read_tap(pixels, w, h, x0 + 1, y0 + 1);

    let c0 = mix(c00, c10, fx);
    let c1 = mix(c01, c11, fx);
    mix(c0, c1, fy)
}

/// Bilinear-only: the nearest path is a near-memcpy that the scalar code already
/// nails, where the vector widen/convert round-trip would only add overhead.
pub(super) fn apply_rgba_bilinear<T: NeonRgba>(
    transform: &Transform,
    input: &Image,
    output: &mut Image,
) {
    let in_w = input.desc.width;
    let in_h = input.desc.height;
    let out_w = output.desc.width;
    let out_stride = output.desc.row_bytes();

    let in_pixels: &[T] = bytemuck::cast_slice(input.bytes());

    let inv = transform.transform.inverse();

    output
        .bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row_bytes)| {
            let out_row: &mut [T] = bytemuck::cast_slice_mut(out_row_bytes);
            for x in 0..out_w {
                let out_pos = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let src = inv.transform_point2(out_pos) - Vec2::splat(0.5);
                let v = sample_bilinear(in_pixels, in_w, in_h, src);
                // SAFETY: `x < out_w` ⇒ `x*4 + 4 <= out_row.len()`.
                unsafe { T::store4(out_row, x * 4, v) };
            }
        });
}
