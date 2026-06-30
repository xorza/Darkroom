//! SSE4.1 bilinear specialization for the packed (RGB/RGBA) transform paths: one
//! pixel fills the low `N` lanes of an `f32x4` (`__m128`), so the convert → mix →
//! convert chain is vectorized across channels — the x86 analog of `neon.rs`. L
//! stays on the scalar path (one useful lane), and the gather-bound nearest path
//! the scalar code already nails.
//!
//! Every step matches the scalar reference bit-for-bit: widening is exact for
//! `0..=255` / `0..=65535`, the blend uses plain muls + adds (no FMA), the clamp
//! is `max`/`min`, and the float→int convert truncates toward zero (`cvtt`) like
//! `as`. The `sse_matches_scalar` test asserts exact equality.
//!
//! All SIMD work flows through `process_row`, which is `#[target_feature(enable =
//! "sse4.1")]`; the `#[inline]` helpers below are pulled into that feature
//! context, so the caller only has to confirm SSE4.1 at runtime before dispatch.

use std::arch::x86_64::*;

use glam::{Affine2, Vec2};
use rayon::prelude::*;

use super::{Transform, TransformElem};
use crate::image::Image;

/// A packed (RGB/RGBA) element type that loads/stores one pixel's `N` channels
/// as the low `N` lanes of an `f32x4`.
pub(super) trait SsePacked: TransformElem {
    /// Loads the `N` channels at element index `base` into the low `N` lanes
    /// (higher lanes are zero).
    ///
    /// # Safety
    /// `base + N <= pixels.len()` and SSE4.1 is available.
    unsafe fn load<const N: usize>(pixels: &[Self], base: usize) -> __m128;

    /// Clamps/converts and stores the low `N` lanes into the channels at `base`.
    ///
    /// # Safety
    /// `base + N <= out.len()` and SSE4.1 is available.
    unsafe fn store<const N: usize>(out: &mut [Self], base: usize, v: __m128);
}

impl SsePacked for u8 {
    #[inline]
    unsafe fn load<const N: usize>(pixels: &[u8], base: usize) -> __m128 {
        unsafe {
            let p = pixels.as_ptr().add(base);
            // Pack the channels into the low bytes of a u32; for N==3 read the 3
            // bytes individually to avoid a 4-byte over-read past the last pixel.
            let packed: u32 = match N {
                4 => (p as *const u32).read_unaligned(),
                3 => *p as u32 | ((*p.add(1) as u32) << 8) | ((*p.add(2) as u32) << 16),
                _ => unreachable!(),
            };
            // Zero-extend the 4 low bytes to 4 i32 lanes, then to f32.
            let i = _mm_cvtepu8_epi32(_mm_cvtsi32_si128(packed as i32));
            _mm_cvtepi32_ps(i)
        }
    }
    #[inline]
    unsafe fn store<const N: usize>(out: &mut [u8], base: usize, v: __m128) {
        unsafe {
            let i = clamp_to_i32(v, 255.0);
            let p = out.as_mut_ptr().add(base);
            match N {
                4 => {
                    // i32 → u16 (saturating) → u8 (saturating); values already in
                    // range, so saturation never triggers. Low 4 bytes are lanes.
                    let u16x = _mm_packus_epi32(i, i);
                    let u8x = _mm_packus_epi16(u16x, u16x);
                    (p as *mut u32).write_unaligned(_mm_cvtsi128_si32(u8x) as u32);
                }
                3 => {
                    *p = _mm_extract_epi32::<0>(i) as u8;
                    *p.add(1) = _mm_extract_epi32::<1>(i) as u8;
                    *p.add(2) = _mm_extract_epi32::<2>(i) as u8;
                }
                _ => unreachable!(),
            }
        }
    }
}

impl SsePacked for u16 {
    #[inline]
    unsafe fn load<const N: usize>(pixels: &[u16], base: usize) -> __m128 {
        unsafe {
            let p = pixels.as_ptr().add(base);
            let u16x = match N {
                4 => _mm_loadl_epi64(p as *const __m128i),
                3 => {
                    let tmp = [*p, *p.add(1), *p.add(2), 0u16];
                    _mm_loadl_epi64(tmp.as_ptr() as *const __m128i)
                }
                _ => unreachable!(),
            };
            _mm_cvtepi32_ps(_mm_cvtepu16_epi32(u16x))
        }
    }
    #[inline]
    unsafe fn store<const N: usize>(out: &mut [u16], base: usize, v: __m128) {
        unsafe {
            let i = clamp_to_i32(v, 65535.0);
            let p = out.as_mut_ptr().add(base);
            match N {
                4 => _mm_storel_epi64(p as *mut __m128i, _mm_packus_epi32(i, i)),
                3 => {
                    *p = _mm_extract_epi32::<0>(i) as u16;
                    *p.add(1) = _mm_extract_epi32::<1>(i) as u16;
                    *p.add(2) = _mm_extract_epi32::<2>(i) as u16;
                }
                _ => unreachable!(),
            }
        }
    }
}

impl SsePacked for f32 {
    #[inline]
    unsafe fn load<const N: usize>(pixels: &[f32], base: usize) -> __m128 {
        unsafe {
            let p = pixels.as_ptr().add(base);
            match N {
                4 => _mm_loadu_ps(p),
                3 => {
                    let tmp = [*p, *p.add(1), *p.add(2), 0.0f32];
                    _mm_loadu_ps(tmp.as_ptr())
                }
                _ => unreachable!(),
            }
        }
    }
    #[inline]
    unsafe fn store<const N: usize>(out: &mut [f32], base: usize, v: __m128) {
        // Unclamped, matching the scalar f32 `from_f32`.
        unsafe {
            let p = out.as_mut_ptr().add(base);
            match N {
                4 => _mm_storeu_ps(p, v),
                3 => {
                    let mut tmp = [0.0f32; 4];
                    _mm_storeu_ps(tmp.as_mut_ptr(), v);
                    *p = tmp[0];
                    *p.add(1) = tmp[1];
                    *p.add(2) = tmp[2];
                }
                _ => unreachable!(),
            }
        }
    }
}

/// Clamps `v` to `[0, max]` and truncates toward zero — matches `from_f32`.
#[inline]
fn clamp_to_i32(v: __m128, max: f32) -> __m128i {
    unsafe {
        _mm_cvttps_epi32(_mm_min_ps(
            _mm_max_ps(v, _mm_setzero_ps()),
            _mm_set1_ps(max),
        ))
    }
}

/// `a*(1-t) + b*t` per lane with a broadcast scalar `t` — two muls + add (no
/// FMA), so the rounding matches the scalar `mix`.
#[inline]
fn mix(a: __m128, b: __m128, t: f32) -> __m128 {
    unsafe {
        _mm_add_ps(
            _mm_mul_ps(a, _mm_set1_ps(1.0 - t)),
            _mm_mul_ps(b, _mm_set1_ps(t)),
        )
    }
}

/// Reads the tap at integer `(x, y)` into the low `N` lanes; out-of-bounds taps
/// read as all-zero, matching the scalar `read_pixel`.
#[inline]
fn read_tap<T: SsePacked, const N: usize>(
    pixels: &[T],
    w: usize,
    h: usize,
    x: i32,
    y: i32,
) -> __m128 {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return unsafe { _mm_setzero_ps() };
    }
    let base = (y as usize * w + x as usize) * N;
    // SAFETY: the bounds check above guarantees `base + N <= pixels.len()`.
    unsafe { T::load::<N>(pixels, base) }
}

pub(super) fn apply_packed<T: SsePacked, const N: usize>(
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
            // SAFETY: the dispatcher in `cpu.rs` confirmed SSE4.1 at runtime.
            unsafe { process_row::<T, N>(in_pixels, in_w, in_h, inv, out_w, y, out_row) };
        });
}

#[target_feature(enable = "sse4.1")]
unsafe fn process_row<T: SsePacked, const N: usize>(
    in_pixels: &[T],
    in_w: usize,
    in_h: usize,
    inv: Affine2,
    out_w: usize,
    y: usize,
    out_row: &mut [T],
) {
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
}
