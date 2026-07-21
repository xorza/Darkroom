//! NEON bilinear specialization for the packed (RGB/RGBA) transform paths: one
//! pixel fills the low `N` lanes of an `f32x4`, so the convert → mix → convert
//! chain is vectorized across channels. L stays on the scalar path — its single
//! channel leaves only one useful lane, and a pixel-parallel variant measured
//! slower (the 4-tap gather dominates and doesn't vectorize).
//!
//! Every step matches the scalar reference bit-for-bit: widening is exact for
//! `0..=255` / `0..=65535`, the blend uses plain muls + adds (no FMA), the clamp
//! is `vmax`/`vmin`, and the float→int convert truncates toward zero like `as`.
//! The `neon_matches_scalar` test asserts exact equality.

use std::arch::aarch64::*;

use glam::Vec2;
use rayon::prelude::*;

use super::{Transform, TransformElem};
use crate::image::Image;

/// A packed (RGB/RGBA) element type that loads/stores one pixel's `N` channels
/// as the low `N` lanes of an `f32x4`.
pub(super) trait NeonPacked: TransformElem {
    /// Loads the `N` channels at element index `base` into the low `N` lanes
    /// (higher lanes are zero).
    ///
    /// # Safety
    /// `base + N <= pixels.len()`.
    unsafe fn load<const N: usize>(pixels: &[Self], base: usize) -> float32x4_t;

    /// Clamps/converts and stores the low `N` lanes into the channels at `base`.
    ///
    /// # Safety
    /// `base + N <= out.len()`.
    unsafe fn store<const N: usize>(out: &mut [Self], base: usize, v: float32x4_t);
}

impl NeonPacked for u8 {
    #[inline]
    unsafe fn load<const N: usize>(pixels: &[u8], base: usize) -> float32x4_t {
        unsafe {
            let p = pixels.as_ptr().add(base);
            // Pack the channels into the low bytes of a u32; for N==3 read the 3
            // bytes individually to avoid a 4-byte over-read past the last pixel.
            let packed: u32 = match N {
                4 => (p as *const u32).read_unaligned(),
                3 => *p as u32 | ((*p.add(1) as u32) << 8) | ((*p.add(2) as u32) << 16),
                _ => unreachable!(),
            };
            let bytes = vcreate_u8(packed as u64);
            vcvtq_f32_u32(vmovl_u16(vget_low_u16(vmovl_u8(bytes))))
        }
    }
    #[inline]
    unsafe fn store<const N: usize>(out: &mut [u8], base: usize, v: float32x4_t) {
        unsafe {
            let u = clamp_to_u32(v, 255.0);
            let p = out.as_mut_ptr().add(base);
            match N {
                4 => {
                    let n16 = vmovn_u32(u);
                    let u8x = vmovn_u16(vcombine_u16(n16, n16));
                    let packed = vget_lane_u32::<0>(vreinterpret_u32_u8(u8x));
                    (p as *mut u32).write_unaligned(packed);
                }
                3 => {
                    *p = vgetq_lane_u32::<0>(u) as u8;
                    *p.add(1) = vgetq_lane_u32::<1>(u) as u8;
                    *p.add(2) = vgetq_lane_u32::<2>(u) as u8;
                }
                _ => unreachable!(),
            }
        }
    }
}

impl NeonPacked for u16 {
    #[inline]
    unsafe fn load<const N: usize>(pixels: &[u16], base: usize) -> float32x4_t {
        unsafe {
            let p = pixels.as_ptr().add(base);
            let u16x = match N {
                4 => vld1_u16(p),
                3 => vld1_u16([*p, *p.add(1), *p.add(2), 0].as_ptr()),
                _ => unreachable!(),
            };
            vcvtq_f32_u32(vmovl_u16(u16x))
        }
    }
    #[inline]
    unsafe fn store<const N: usize>(out: &mut [u16], base: usize, v: float32x4_t) {
        unsafe {
            let u = clamp_to_u32(v, 65535.0);
            let p = out.as_mut_ptr().add(base);
            match N {
                4 => vst1_u16(p, vmovn_u32(u)),
                3 => {
                    *p = vgetq_lane_u32::<0>(u) as u16;
                    *p.add(1) = vgetq_lane_u32::<1>(u) as u16;
                    *p.add(2) = vgetq_lane_u32::<2>(u) as u16;
                }
                _ => unreachable!(),
            }
        }
    }
}

impl NeonPacked for f32 {
    #[inline]
    unsafe fn load<const N: usize>(pixels: &[f32], base: usize) -> float32x4_t {
        unsafe {
            let p = pixels.as_ptr().add(base);
            match N {
                4 => vld1q_f32(p),
                3 => vld1q_f32([*p, *p.add(1), *p.add(2), 0.0].as_ptr()),
                _ => unreachable!(),
            }
        }
    }
    #[inline]
    unsafe fn store<const N: usize>(out: &mut [f32], base: usize, v: float32x4_t) {
        // Unclamped, matching the scalar f32 `from_f32`.
        unsafe {
            let p = out.as_mut_ptr().add(base);
            match N {
                4 => vst1q_f32(p, v),
                3 => {
                    *p = vgetq_lane_f32::<0>(v);
                    *p.add(1) = vgetq_lane_f32::<1>(v);
                    *p.add(2) = vgetq_lane_f32::<2>(v);
                }
                _ => unreachable!(),
            }
        }
    }
}

/// Clamps `v` to `[0, max]` and truncates toward zero — matches `from_f32`.
#[inline]
fn clamp_to_u32(v: float32x4_t, max: f32) -> uint32x4_t {
    unsafe { vcvtq_u32_f32(vminq_f32(vmaxq_f32(v, vdupq_n_f32(0.0)), vdupq_n_f32(max))) }
}

/// `a*(1-t) + b*t` per lane with a broadcast scalar `t` — two muls + add (no
/// FMA), so the rounding matches the scalar `mix`.
#[inline]
fn mix(a: float32x4_t, b: float32x4_t, t: f32) -> float32x4_t {
    unsafe { vaddq_f32(vmulq_n_f32(a, 1.0 - t), vmulq_n_f32(b, t)) }
}

#[inline]
fn read_tap<T: NeonPacked, const N: usize>(
    pixels: &[T],
    w: usize,
    h: usize,
    x: i32,
    y: i32,
) -> float32x4_t {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return unsafe { vdupq_n_f32(0.0) };
    }
    let base = (y as usize * w + x as usize) * N;
    // SAFETY: the bounds check above guarantees `base + N <= pixels.len()`.
    unsafe { T::load::<N>(pixels, base) }
}

pub(super) fn apply_packed<T: NeonPacked, const N: usize>(
    transform: &Transform,
    input: &Image,
    output: &mut Image,
) {
    let in_w = input.desc().width;
    let in_h = input.desc().height;
    let out_w = output.desc().width;
    let out_stride = output.desc().row_bytes();

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
                let x0f = src.x.floor();
                let y0f = src.y.floor();
                let fx = src.x - x0f;
                let fy = src.y - y0f;
                let (x0, y0) = (x0f as i32, y0f as i32);

                let c00 = read_tap::<T, N>(in_pixels, in_w, in_h, x0, y0);
                let c10 = read_tap::<T, N>(in_pixels, in_w, in_h, x0 + 1, y0);
                let c01 = read_tap::<T, N>(in_pixels, in_w, in_h, x0, y0 + 1);
                let c11 = read_tap::<T, N>(in_pixels, in_w, in_h, x0 + 1, y0 + 1);

                let v = mix(mix(c00, c10, fx), mix(c01, c11, fx), fy);
                // SAFETY: `x < out_w` ⇒ `x*N + N <= out_row.len()`.
                unsafe { T::store::<N>(out_row, x * N, v) };
            }
        });
}
