//! Optimized row-warping implementations.
//!
//! Runtime dispatch to the best available implementation:
//! - **Bilinear**: AVX2/SSE4.1 on x86_64, NEON on aarch64, scalar with incremental stepping
//!   elsewhere
//! - **Lanczos2/3/4**: incremental stepping, fast-path interior bounds skipping, and a SIMD
//!   interior kernel — x86_64 AVX2/FMA and aarch64 NEON. On x86 the Lanczos3/4 (SIZE=6/8) kernel
//!   is 256-bit (one `__m256` load/accumulate per row) and the per-pixel tap weights come from a
//!   vector `i32gather` of the LUT; SIZE=4 and aarch64 use the 128-bit kernel and scalar weight
//!   lookups.
//!
//! When SIP distortion correction is active, incremental stepping is disabled
//! (SIP is nonlinear) and SIMD paths fall back to scalar.

use crate::math::size2us::Size2us;
#[cfg(target_arch = "aarch64")]
use crate::simd::NEON_F32_LANES;
use crate::simd::dispatch;
#[cfg(target_arch = "x86_64")]
use crate::simd::{AVX2_F32_LANES, SSE_F32_LANES};

#[cfg(target_arch = "x86_64")]
mod x86;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(test)]
mod tests;

use crate::stacking::registration::config::WarpParams;
#[cfg(target_arch = "x86_64")]
use crate::stacking::registration::resample::kernel::LANCZOS_LUT_RESOLUTION;
use crate::stacking::registration::resample::kernel::{self, LanczosLut};
use crate::stacking::registration::transform::WarpTransform;
use glam::{DVec2, Vec2};
use imaginarium::Buffer2;

#[inline]
fn finite_source_position(src_x: f64, src_y: f64) -> Option<Vec2> {
    let pos = Vec2::new(src_x as f32, src_y as f32);
    pos.is_finite().then_some(pos)
}

#[inline]
fn source_position_in_footprint(
    src_x: f64,
    src_y: f64,
    input_width: usize,
    input_height: usize,
) -> Option<Vec2> {
    let pos = finite_source_position(src_x, src_y)?;
    kernel::source_footprint_contains(pos, Size2us::new(input_width, input_height)).then_some(pos)
}

#[inline]
pub(super) fn for_each_source_position(
    output_y: usize,
    wt: &WarpTransform,
    output_width: usize,
    mut visit: impl FnMut(usize, Option<Vec2>),
) {
    let m = wt.transform.matrix();
    let can_step = wt.is_linear();
    let src0 = wt.apply(DVec2::new(0.0, output_y as f64));
    let mut src_x = src0.x;
    let mut src_y = src0.y;
    let dx_step = m[0];
    let dy_step = m[3];

    for x in 0..output_width {
        if !can_step {
            let src = wt.apply(DVec2::new(x as f64, output_y as f64));
            src_x = src.x;
            src_y = src.y;
        }
        visit(x, finite_source_position(src_x, src_y));
        if can_step {
            src_x += dx_step;
            src_y += dy_step;
        }
    }
}

/// Inverse-map one output row to source coordinates and fill it via `sample`.
///
/// Centralizes the linear incremental-stepping fast path — for non-perspective transforms the
/// source coordinate advances by a constant `(m[0], m[3])` per output pixel, so the per-pixel matrix
/// multiply is skipped. Shared by the scalar bilinear, generic per-pixel, and quality-map row loops
/// so value and support coordinates cannot drift. Non-finite projected coordinates produce
/// `invalid_value` without reaching a sampler.
#[inline]
pub(super) fn sample(
    output_y: usize,
    wt: &WarpTransform,
    output_row: &mut [f32],
    invalid_value: f32,
    mut sample: impl FnMut(Vec2) -> f32,
) {
    for_each_source_position(output_y, wt, output_row.len(), |x, pos| {
        output_row[x] = pos.map_or(invalid_value, &mut sample);
    });
}

/// Warp a row of pixels using bilinear interpolation.
///
/// Uses AVX2/SSE4.1 on x86_64, NEON on aarch64, scalar with incremental stepping elsewhere.
/// When SIP is active, falls back to scalar (SIP is nonlinear).
#[inline]
pub(super) fn bilinear(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    border_value: f32,
) {
    // Every kernel hardcodes a 0.0 border and none of them models SIP's nonlinearity, so either
    // condition sends the row to the scalar path however wide it is.
    if wt.has_sip() || border_value != 0.0 {
        return bilinear_scalar(input, output_row, output_y, wt, border_value);
    }

    // Each kernel consumes one vector of output pixels per chunk (`output_width / LANES` chunks),
    // so a row shorter than one vector gives the chunk loop nothing to do and every pixel falls
    // through to the kernel's own remainder handling — cheaper to take the scalar row instead.
    // A structural minimum, not a measured crossover: these kernels win from one vector up.
    dispatch! {
        x86: avx2 if output_row.len() >= AVX2_F32_LANES
            => x86::bilinear_avx2(input, output_row, output_y, &wt.transform),
        x86: sse4_1 if output_row.len() >= SSE_F32_LANES
            => x86::bilinear_sse(input, output_row, output_y, &wt.transform),
        aarch64 if output_row.len() >= NEON_F32_LANES
            => neon::bilinear_neon(input, output_row, output_y, &wt.transform),
        scalar => bilinear_scalar(input, output_row, output_y, wt, border_value),
    }
}

/// Scalar implementation of row warping with bilinear interpolation.
///
/// Uses incremental coordinate stepping for linear transforms (no SIP, no perspective)
/// to avoid per-pixel matrix multiply.
fn bilinear_scalar(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    border_value: f32,
) {
    sample(output_y, wt, output_row, border_value, |pos| {
        kernel::bilinear_sample(input, pos, border_value)
    });
}

/// Optimized Lanczos row warping with incremental coordinate stepping.
///
/// Dispatches to the appropriate monomorphization based on `lanczos_param()`. Works for Lanczos2
/// (4×4), Lanczos3 (6×6), and Lanczos4 (8×8).
///
/// Key optimizations:
/// 1. Incremental source coordinate stepping (avoid per-pixel matrix multiply)
/// 2. Fast-path for interior pixels (skip bounds checks, use direct row pointers)
/// 3. SIMD tap-weight computation: x86 gathers the LUT for Lanczos3/4; otherwise scalar lookups
/// 4. SIMD interior fast path: x86_64 AVX2/FMA (256-bit for Lanczos3/4, 128-bit for Lanczos2),
///    aarch64 NEON (128-bit, all sizes)
pub(super) fn lanczos(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    params: &WarpParams,
) {
    let a = params.method.lanczos_param().unwrap();
    match a {
        2 => lanczos_inner::<2, 4>(input, output_row, output_y, wt, params),
        3 => lanczos_inner::<3, 6>(input, output_row, output_y, wt, params),
        4 => lanczos_inner::<4, 8>(input, output_row, output_y, wt, params),
        _ => panic!("Unsupported Lanczos parameter: {a}"),
    }
}

/// The `SIZE` separable Lanczos tap weights for fractional offset `frac`.
///
/// The gather kernel only pays off once ≥ 6 taps amortize its 8-wide gather, so Lanczos2 stays
/// scalar — the gather measured ~6% slower there. `SIZE > 4` is const, so the guard folds away.
#[inline]
fn lanczos_weights<const A: usize, const SIZE: usize>(lut: &LanczosLut, frac: f32) -> [f32; SIZE] {
    dispatch! {
        x86: avx2_fma if SIZE > 4 => x86::lanczos_weights_gather::<A, SIZE>(
            lut.values.as_ptr(),
            LANCZOS_LUT_RESOLUTION as f32,
            frac,
        ),
        scalar => lanczos_weights_scalar::<A, SIZE>(lut, frac),
    }
}

/// Scalar Lanczos tap weights for fractional offset `frac` (non-x86 / no-AVX2 fallback for
/// [`x86::lanczos_weights_gather`]). Same distance convention as the gather helper.
///
/// For `i < A` the distance is `(A-1-i) + frac ∈ [0, A)`; for `i ≥ A` it is `(i-A+1) - frac ∈
/// (0, A]` — both non-negative, hence `lookup_positive`.
#[inline]
fn lanczos_weights_scalar<const A: usize, const SIZE: usize>(
    lut: &LanczosLut,
    frac: f32,
) -> [f32; SIZE] {
    let a_minus_1 = A as i32 - 1;
    let mut w = [0.0f32; SIZE];
    for (i, wi) in w.iter_mut().enumerate() {
        *wi = if i < A {
            lut.lookup_positive((a_minus_1 - i as i32) as f32 + frac)
        } else {
            lut.lookup_positive((i as i32 - a_minus_1) as f32 - frac)
        };
    }
    w
}

/// Vector accumulation of the `SIZE`×`SIZE` weighted window whose top-left tap is `(kx0, ky0)`.
///
/// `None` when the target has no vector backend, or when the window's loads would leave the
/// image — the SIMD bounds are wider than the scalar loop's, so the caller has to re-test before
/// taking its own fast path. x86 loads 8 floats per row for Lanczos3/4 (SIZE=6 zero-pads two) and
/// NEON walks that same 8-wide window as a 128-bit lo+hi pair, so both need `kx0 + 8 ≤ width`
/// where the scalar loop needs only `kx0 + SIZE ≤ width`.
#[inline]
fn lanczos_accumulate<const SIZE: usize>(
    input: &Buffer2<f32>,
    kx0: i32,
    ky0: i32,
    wx: &[f32; SIZE],
    wy: &[f32; SIZE],
) -> Option<f32> {
    let simd_cols: i32 = if SIZE > 4 { 8 } else { SIZE as i32 };
    if kx0 < 0
        || ky0 < 0
        || kx0 + simd_cols > input.width() as i32
        || ky0 + SIZE as i32 > input.height() as i32
    {
        return None;
    }

    let pixels = input.pixels();
    let width = input.width();
    let kx = kx0 as usize;
    let ky = ky0 as usize;

    dispatch! {
        x86: avx2_fma => Some(x86::lanczos_kernel_fma::<SIZE>(pixels, width, kx, ky, wx, wy)),
        aarch64 => Some(neon::lanczos_kernel_neon::<SIZE>(pixels, width, kx, ky, wx, wy)),
        scalar => None,
    }
}

fn lanczos_inner<const A: usize, const SIZE: usize>(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    params: &WarpParams,
) {
    let pixels = input.pixels();
    let input_width = input.width();
    let input_height = input.height();
    let border_value = params.border_value;

    let lut = kernel::get_lanczos_lut(A);
    let iw = input_width as i32;
    let ih = input_height as i32;
    let a_minus_1 = A as i32 - 1;

    let m = wt.transform.matrix();
    let can_step = wt.is_linear();

    let src0 = wt.apply(DVec2::new(0.0, output_y as f64));
    let mut src_x = src0.x;
    let mut src_y = src0.y;
    let dx_step = m[0];
    let dy_step = m[3];

    for (x_idx, out_pixel) in output_row.iter_mut().enumerate() {
        if !can_step {
            let src = wt.apply(DVec2::new(x_idx as f64, output_y as f64));
            src_x = src.x;
            src_y = src.y;
        }

        let Some(pos) = source_position_in_footprint(src_x, src_y, input_width, input_height)
        else {
            *out_pixel = border_value;
            if can_step {
                src_x += dx_step;
                src_y += dy_step;
            }
            continue;
        };
        let sx = pos.x;
        let sy = pos.y;

        let x0 = kernel::fast_floor_i32(sx);
        let y0 = kernel::fast_floor_i32(sy);
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;

        let kx0 = x0 - a_minus_1;
        let ky0 = y0 - a_minus_1;

        let wx = lanczos_weights::<A, SIZE>(lut, fx);
        let wy = lanczos_weights::<A, SIZE>(lut, fy);

        let total_sum = wx.iter().sum::<f32>() * wy.iter().sum::<f32>();
        let inv_total = if total_sum.abs() > 1e-10 {
            1.0 / total_sum
        } else {
            1.0
        };

        if let Some(acc) = lanczos_accumulate::<SIZE>(input, kx0, ky0, &wx, &wy) {
            *out_pixel = acc * inv_total;
        } else if kx0 >= 0 && ky0 >= 0 && kx0 + SIZE as i32 - 1 < iw && ky0 + SIZE as i32 - 1 < ih {
            // Scalar fast path: all SIZE×SIZE pixels in bounds, direct indexing
            let kx = kx0 as usize;
            let ky = ky0 as usize;
            let w = input_width;

            let mut acc = 0.0f32;
            for (j, &wyj) in wy.iter().enumerate() {
                let row_off = (ky + j) * w + kx;
                for (k, &wxk) in wx.iter().enumerate() {
                    let v = unsafe { *pixels.get_unchecked(row_off + k) };
                    acc += v * wxk * wyj;
                }
            }
            *out_pixel = acc * inv_total;
        } else {
            // Truncating a signed Lanczos kernel can make its remaining weight arbitrarily close
            // to zero, so partial support uses stable edge-extended bilinear interpolation.
            *out_pixel = kernel::bilinear_sample(input, pos, border_value);
        }

        if can_step {
            src_x += dx_step;
            src_y += dy_step;
        }
    }
}
