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

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub(crate) mod sse;

#[cfg(target_arch = "aarch64")]
pub(crate) mod neon;

#[cfg(target_arch = "x86_64")]
use crate::stacking::registration::interpolation::LANCZOS_LUT_RESOLUTION;
use crate::stacking::registration::interpolation::LanczosLut;
use crate::stacking::registration::interpolation::WarpParams;
use crate::stacking::registration::interpolation::{
    bilinear_sample, bilinear_sample_edge_extended, fast_floor_i32, get_lanczos_lut,
};
use crate::stacking::registration::transform::WarpTransform;
use glam::{DVec2, Vec2};
use imaginarium::Buffer2;

#[inline]
fn finite_source_position(src_x: f64, src_y: f64) -> Option<Vec2> {
    let pos = Vec2::new(src_x as f32, src_y as f32);
    pos.is_finite().then_some(pos)
}

#[inline]
fn lanczos_source_position<const A: usize>(
    src_x: f64,
    src_y: f64,
    input_width: usize,
    input_height: usize,
) -> Option<Vec2> {
    let pos = finite_source_position(src_x, src_y)?;
    let support_min = -(A as f32);
    let support_max_x = input_width as f32 + A as f32 - 1.0;
    let support_max_y = input_height as f32 + A as f32 - 1.0;

    // Reject no-overlap coordinates before floor and LUT index arithmetic.
    (pos.x >= support_min && pos.y >= support_min && pos.x < support_max_x && pos.y < support_max_y)
        .then_some(pos)
}

#[inline]
pub(crate) fn for_each_source_position(
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
pub(crate) fn warp_row_with(
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
pub(crate) fn warp_row_bilinear(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    border_value: f32,
) {
    // SIMD paths use hardcoded 0.0 border — only use them when border_value is 0.0
    #[cfg(target_arch = "x86_64")]
    if !wt.has_sip() && border_value == 0.0 {
        let output_width = output_row.len();
        if output_width >= 8 && cpu_features::has_avx2() {
            unsafe {
                sse::warp_row_bilinear_avx2(input, output_row, output_y, &wt.transform);
            }
            return;
        }
        if output_width >= 4 && cpu_features::has_sse4_1() {
            unsafe {
                sse::warp_row_bilinear_sse(input, output_row, output_y, &wt.transform);
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    if !wt.has_sip() && border_value == 0.0 && output_row.len() >= 4 {
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            neon::warp_row_bilinear_neon(input, output_row, output_y, &wt.transform);
        }
        return;
    }

    // Scalar fallback (also used when SIP is active or the row is too short for SIMD)
    warp_row_bilinear_scalar(input, output_row, output_y, wt, border_value);
}

/// Scalar implementation of row warping with bilinear interpolation.
///
/// Uses incremental coordinate stepping for linear transforms (no SIP, no perspective)
/// to avoid per-pixel matrix multiply.
pub(crate) fn warp_row_bilinear_scalar(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    border_value: f32,
) {
    warp_row_with(output_y, wt, output_row, border_value, |pos| {
        bilinear_sample(input, pos, border_value)
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
pub(crate) fn warp_row_lanczos(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    params: &WarpParams,
) {
    let a = params.method.lanczos_param().unwrap();
    match a {
        2 => warp_row_lanczos_inner::<2, 4>(input, output_row, output_y, wt, params),
        3 => warp_row_lanczos_inner::<3, 6>(input, output_row, output_y, wt, params),
        4 => warp_row_lanczos_inner::<4, 8>(input, output_row, output_y, wt, params),
        _ => panic!("Unsupported Lanczos parameter: {a}"),
    }
}

/// Scalar Lanczos tap weights for fractional offset `frac` (non-x86 / no-AVX2 fallback for
/// [`sse::lanczos_weights_gather`]). Same distance convention as the gather helper.
#[inline]
fn lanczos_weights_scalar<const A: usize, const SIZE: usize>(
    lut: &LanczosLut,
    a_minus_1: i32,
    frac: f32,
) -> [f32; SIZE] {
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

fn warp_row_lanczos_inner<const A: usize, const SIZE: usize>(
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

    let lut = get_lanczos_lut(A);
    let iw = input_width as i32;
    let ih = input_height as i32;
    let a_minus_1 = A as i32 - 1;

    #[cfg(target_arch = "x86_64")]
    let use_fma = cpu_features::has_avx2_fma();

    // Per-tap distance affine `dist[i] = base[i] + sign[i]·frac` for the SIMD weight gather (x86).
    // Lanes `SIZE..8` stay zero → index 0 (in-bounds dummy). Loop-invariant in A/SIZE — built once.
    #[cfg(target_arch = "x86_64")]
    let (lut_base, lut_sign) = {
        let mut base = [0.0f32; 8];
        let mut sign = [0.0f32; 8];
        for i in 0..SIZE {
            if i < A {
                base[i] = (a_minus_1 - i as i32) as f32;
                sign[i] = 1.0;
            } else {
                base[i] = (i as i32 - a_minus_1) as f32;
                sign[i] = -1.0;
            }
        }
        (base, sign)
    };

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

        let Some(pos) = lanczos_source_position::<A>(src_x, src_y, input_width, input_height)
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

        let x0 = fast_floor_i32(sx);
        let y0 = fast_floor_i32(sy);
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;

        let kx0 = x0 - a_minus_1;
        let ky0 = y0 - a_minus_1;

        // Kernel weights. For i < A: distance = (A-1-i) + frac ∈ [0, A); for i ≥ A:
        // distance = (i - A+1) - frac ∈ (0, A] — both non-negative, so `lookup_positive`.
        // Gather pays off only when ≥ 6 taps amortize the 8-wide gather; SIZE=4 (Lanczos2) stays
        // scalar (the gather measured ~6% slower there). `SIZE > 4` is const.
        #[cfg(target_arch = "x86_64")]
        let (wx, wy): ([f32; SIZE], [f32; SIZE]) = if use_fma && SIZE > 4 {
            let res = LANCZOS_LUT_RESOLUTION as f32;
            let ptr = lut.values.as_ptr();
            unsafe {
                (
                    sse::lanczos_weights_gather::<SIZE>(ptr, &lut_base, &lut_sign, res, fx),
                    sse::lanczos_weights_gather::<SIZE>(ptr, &lut_base, &lut_sign, res, fy),
                )
            }
        } else {
            (
                lanczos_weights_scalar::<A, SIZE>(lut, a_minus_1, fx),
                lanczos_weights_scalar::<A, SIZE>(lut, a_minus_1, fy),
            )
        };
        #[cfg(not(target_arch = "x86_64"))]
        let (wx, wy): ([f32; SIZE], [f32; SIZE]) = (
            lanczos_weights_scalar::<A, SIZE>(lut, a_minus_1, fx),
            lanczos_weights_scalar::<A, SIZE>(lut, a_minus_1, fy),
        );

        let total_sum = wx.iter().sum::<f32>() * wy.iter().sum::<f32>();
        let inv_total = if total_sum.abs() > 1e-10 {
            1.0 / total_sum
        } else {
            1.0
        };

        // SIMD fast path (x86_64 AVX2+FMA / aarch64 NEON): needs all SIZE rows and enough columns for
        // the loads. x86 Lanczos3/4 uses one 256-bit (8-wide) load/row; SIZE=4 and NEON use 128-bit.
        // SIZE=4: reads 4 floats/row, needs kx0 + 3 < iw
        // SIZE=6/8: reads 8 floats/row (SIZE=6 zero-pads 2), needs kx0 + 7 < iw
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            let simd_cols: i32 = if SIZE > 4 { 8 } else { SIZE as i32 };
            let in_bounds =
                kx0 >= 0 && ky0 >= 0 && kx0 + simd_cols - 1 < iw && ky0 + SIZE as i32 - 1 < ih;
            // x86 AVX2/FMA is runtime-detected; NEON is mandatory on aarch64.
            #[cfg(target_arch = "x86_64")]
            let try_simd = use_fma && in_bounds;
            #[cfg(target_arch = "aarch64")]
            let try_simd = in_bounds;

            if try_simd {
                let acc = unsafe {
                    #[cfg(target_arch = "x86_64")]
                    {
                        sse::lanczos_kernel_fma::<SIZE>(
                            pixels,
                            input_width,
                            kx0 as usize,
                            ky0 as usize,
                            &wx,
                            &wy,
                        )
                    }
                    #[cfg(target_arch = "aarch64")]
                    {
                        neon::lanczos_kernel_neon::<SIZE>(
                            pixels,
                            input_width,
                            kx0 as usize,
                            ky0 as usize,
                            &wx,
                            &wy,
                        )
                    }
                };
                *out_pixel = acc * inv_total;

                if can_step {
                    src_x += dx_step;
                    src_y += dy_step;
                }
                continue;
            }
        }

        if kx0 >= 0 && ky0 >= 0 && kx0 + SIZE as i32 - 1 < iw && ky0 + SIZE as i32 - 1 < ih {
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
            *out_pixel = bilinear_sample_edge_extended(input, pos);
        }

        if can_step {
            src_x += dx_step;
            src_y += dy_step;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::stacking::registration::config::InterpolationMethod;
    use crate::stacking::registration::interpolation::WarpParams;
    use crate::stacking::registration::interpolation::warp::*;
    use crate::stacking::registration::transform::Transform;
    use crate::testing::synthetic::patterns;

    /// Naive scalar Lanczos3 row warp used as reference for testing the optimized version.
    fn warp_row_lanczos_scalar(
        input: &Buffer2<f32>,
        output_row: &mut [f32],
        output_y: usize,
        wt: &WarpTransform,
    ) {
        let pixels = input.pixels();
        let input_width = input.width();
        let input_height = input.height();

        let y = output_y as f64;
        const A: usize = 3;

        for (x, out_pixel) in output_row.iter_mut().enumerate() {
            let src = wt.apply(DVec2::new(x as f64, y));
            let Some(pos) = lanczos_source_position::<A>(src.x, src.y, input_width, input_height)
            else {
                *out_pixel = 0.0;
                continue;
            };
            let sx = pos.x;
            let sy = pos.y;

            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            let kx0 = x0 - 2;
            let ky0 = y0 - 2;

            if kx0 < 0 || ky0 < 0 || kx0 + 5 >= input_width as i32 || ky0 + 5 >= input_height as i32
            {
                *out_pixel = bilinear_sample_edge_extended(input, Vec2::new(sx, sy));
                continue;
            }

            let lut = get_lanczos_lut(A);

            let mut wx = [0.0f32; 6];
            for (i, w) in wx.iter_mut().enumerate() {
                let dx = fx - (i as i32 - 2) as f32;
                *w = lut.lookup(dx);
            }

            let mut wy = [0.0f32; 6];
            for (j, w) in wy.iter_mut().enumerate() {
                let dy = fy - (j as i32 - 2) as f32;
                *w = lut.lookup(dy);
            }

            let mut sum = 0.0f32;
            let mut w_in = 0.0f32;
            for (j, &wyj) in wy.iter().enumerate() {
                let py = y0 - 2 + j as i32;
                for (i, &wxi) in wx.iter().enumerate() {
                    let px = x0 - 2 + i as i32;
                    let weight = wxi * wyj;
                    sum += pixels[py as usize * input_width + px as usize] * weight;
                    w_in += weight;
                }
            }

            *out_pixel = if w_in.abs() < 1e-10 { 0.0 } else { sum / w_in };
        }
    }

    #[test]
    fn test_warp_row_bilinear_identity() {
        let width = 100;
        let height = 100;
        let input = patterns::diagonal_gradient(width, height);
        let identity = WarpTransform::new(Transform::identity());

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_bilinear(&input, &mut output_row, y, &identity, 0.0);

        // With identity transform, output should match input
        for x in 1..width - 1 {
            let expected = input[(x, y)];
            assert!(
                (output_row[x] - expected).abs() < 0.01,
                "Mismatch at x={}: {} vs {}",
                x,
                output_row[x],
                expected
            );
        }
    }

    #[test]
    fn test_warp_row_bilinear_translation() {
        let width = 100;
        let height = 100;
        let input = patterns::diagonal_gradient(width, height);

        // Translate by (5, 3)
        let transform = Transform::translation(DVec2::new(5.0, 3.0));
        let inverse = WarpTransform::new(transform.inverse());

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_bilinear(&input, &mut output_row, y, &inverse, 0.0);

        // Check that pixels are shifted
        for (x, &output_val) in output_row.iter().enumerate().skip(10).take(width - 20) {
            // Output at (x, y) should come from input at (x-5, y-3)
            let src_x = x as i32 - 5;
            let src_y = y as i32 - 3;
            if src_x >= 0 && src_y >= 0 {
                let expected = input[(src_x as usize, src_y as usize)];
                assert!(
                    (output_val - expected).abs() < 0.01,
                    "Mismatch at x={}: {} vs {}",
                    x,
                    output_val,
                    expected
                );
            }
        }
    }

    #[test]
    fn test_warp_row_simd_matches_scalar() {
        let width = 128;
        let height = 128;
        let input = patterns::diagonal_gradient(width, height);

        // Test with various transforms
        let transforms = vec![
            Transform::identity(),
            Transform::translation(DVec2::new(2.5, 1.7)),
            Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
        ];

        for transform in transforms {
            let inverse = WarpTransform::new(transform.inverse());

            for y in [0, 50, height - 1] {
                let mut output_simd = vec![0.0f32; width];
                let mut output_scalar = vec![0.0f32; width];

                warp_row_bilinear(&input, &mut output_simd, y, &inverse, 0.0);
                warp_row_bilinear_scalar(&input, &mut output_scalar, y, &inverse, 0.0);

                for x in 0..width {
                    // Tolerance slightly relaxed for SIMD/scalar differences due to
                    // different operation ordering and floating point precision
                    assert!(
                        (output_simd[x] - output_scalar[x]).abs() < 1e-4,
                        "Row {}, x={}: SIMD {} vs Scalar {}",
                        y,
                        x,
                        output_simd[x],
                        output_scalar[x]
                    );
                }
            }
        }
    }

    #[test]
    fn test_warp_row_various_sizes() {
        let height = 64;
        let input_base = patterns::diagonal_gradient(256, height);

        // Test various widths including non-SIMD-aligned sizes
        for width in [
            1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 100, 128,
        ] {
            let input = Buffer2::new(
                width,
                height,
                input_base
                    .pixels()
                    .iter()
                    .take(width * height)
                    .copied()
                    .collect(),
            );
            let identity = WarpTransform::new(Transform::identity());

            let mut output_simd = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            let y = height / 2;

            warp_row_bilinear(&input, &mut output_simd, y, &identity, 0.0);
            warp_row_bilinear_scalar(&input, &mut output_scalar, y, &identity, 0.0);

            for x in 0..width {
                assert!(
                    (output_simd[x] - output_scalar[x]).abs() < 1e-5,
                    "Width {}, x={}: SIMD {} vs Scalar {}",
                    width,
                    x,
                    output_simd[x],
                    output_scalar[x]
                );
            }
        }
    }

    #[test]
    fn test_warp_row_lanczos_scalar_identity() {
        let width = 100;
        let height = 100;
        let input = patterns::diagonal_gradient(width, height);
        let identity = WarpTransform::new(Transform::identity());

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_lanczos_scalar(&input, &mut output_row, y, &identity);

        // With identity transform, output should match input (within Lanczos ringing tolerance)
        for x in 3..width - 3 {
            let expected = input[(x, y)];
            assert!(
                (output_row[x] - expected).abs() < 0.02,
                "Mismatch at x={}: {} vs {}",
                x,
                output_row[x],
                expected
            );
        }
    }

    #[test]
    fn test_warp_row_lanczos_scalar_various_sizes_match_optimized() {
        // Verify scalar and optimized Lanczos3 produce identical results across widths.
        // This replaces a weaker test that only checked is_finite().
        let height = 64;
        let input_base = patterns::diagonal_gradient(256, height);
        let params = WarpParams::new(InterpolationMethod::Lanczos3);

        for width in [1, 2, 3, 4, 5, 7, 8, 16, 33, 64, 100] {
            let input = Buffer2::new(
                width,
                height,
                input_base
                    .pixels()
                    .iter()
                    .take(width * height)
                    .copied()
                    .collect(),
            );
            let transform = Transform::translation(DVec2::new(1.5, 0.5));
            let inverse = WarpTransform::new(transform.inverse());

            let mut output_scalar = vec![0.0f32; width];
            let mut output_fast = vec![0.0f32; width];
            let y = height / 2;

            warp_row_lanczos_scalar(&input, &mut output_scalar, y, &inverse);
            warp_row_lanczos(&input, &mut output_fast, y, &inverse, &params);

            for x in 0..width {
                assert!(
                    (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                    "Width {width}, x={x}: fast {} vs scalar {}",
                    output_fast[x],
                    output_scalar[x]
                );
            }
        }
    }

    #[test]
    fn test_warp_row_lanczos_matches_scalar() {
        let width = 128;
        let height = 128;
        let input = patterns::diagonal_gradient(width, height);
        // Disable clamping to match unclamped scalar reference
        let params = WarpParams::new(InterpolationMethod::Lanczos3);

        let transforms = vec![
            Transform::identity(),
            Transform::translation(DVec2::new(2.5, 1.7)),
            Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
        ];

        for transform in transforms {
            let inverse = WarpTransform::new(transform.inverse());

            for y in [0, 50, height - 1] {
                let mut output_fast = vec![0.0f32; width];
                let mut output_scalar = vec![0.0f32; width];

                warp_row_lanczos(&input, &mut output_fast, y, &inverse, &params);
                warp_row_lanczos_scalar(&input, &mut output_scalar, y, &inverse);

                for x in 0..width {
                    assert!(
                        (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                        "Row {y}, x={x}: fast {} vs scalar {}",
                        output_fast[x],
                        output_scalar[x]
                    );
                }
            }
        }
    }

    #[test]
    fn test_lanczos_preserves_signed_constants_at_interior_and_edges() {
        let (width, height) = (24, 20);
        let identity = WarpTransform::new(Transform::identity());

        for method in [
            InterpolationMethod::Lanczos2,
            InterpolationMethod::Lanczos3,
            InterpolationMethod::Lanczos4,
        ] {
            let params = WarpParams::new(method);
            for expected in [-1.25, 0.0, 2.5] {
                let input = patterns::uniform(width, height, expected);
                for y in [0, 1, 4, 10, height - 1] {
                    let mut output = vec![0.0; width];
                    warp_row_lanczos(&input, &mut output, y, &identity, &params);
                    for (x, actual) in output.into_iter().enumerate() {
                        assert!(
                            (actual - expected).abs() < 2e-5,
                            "{method:?} ({x}, {y}): expected {expected}, got {actual}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_lanczos_is_translation_invariant_for_signed_data() {
        let (width, height) = (24, 20);
        let input = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| ((i * 13 + i / width * 7) % 31) as f32 / 9.0 - 1.7)
                .collect(),
        );
        let offset = 2.25;
        let shifted = Buffer2::new(
            width,
            height,
            input.pixels().iter().map(|value| value + offset).collect(),
        );
        let inverse = WarpTransform::new(Transform::translation(DVec2::new(0.37, -0.43)).inverse());

        for method in [
            InterpolationMethod::Lanczos2,
            InterpolationMethod::Lanczos3,
            InterpolationMethod::Lanczos4,
        ] {
            let params = WarpParams::new(method);
            for y in [0, 1, 5, 10, height - 1] {
                let mut output = vec![0.0; width];
                let mut shifted_output = vec![0.0; width];
                warp_row_lanczos(&input, &mut output, y, &inverse, &params);
                warp_row_lanczos(&shifted, &mut shifted_output, y, &inverse, &params);
                for x in 1..width {
                    let actual_offset = shifted_output[x] - output[x];
                    assert!(
                        (actual_offset - offset).abs() < 5e-5,
                        "{method:?} ({x}, {y}): expected offset {offset}, got {actual_offset}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_warp_row_lanczos_various_sizes() {
        let height = 64;
        let input_base = patterns::diagonal_gradient(256, height);
        // Disable clamping to match unclamped scalar reference
        let params = WarpParams::new(InterpolationMethod::Lanczos3);

        for width in [1, 2, 3, 7, 8, 16, 33, 64, 100] {
            let input = Buffer2::new(
                width,
                height,
                input_base
                    .pixels()
                    .iter()
                    .take(width * height)
                    .copied()
                    .collect(),
            );
            let transform = Transform::translation(DVec2::new(1.5, 0.5));
            let inverse = WarpTransform::new(transform.inverse());

            let mut output_fast = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            let y = height / 2;

            warp_row_lanczos(&input, &mut output_fast, y, &inverse, &params);
            warp_row_lanczos_scalar(&input, &mut output_scalar, y, &inverse);

            for x in 0..width {
                assert!(
                    (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                    "Width {width}, x={x}: fast {} vs scalar {}",
                    output_fast[x],
                    output_scalar[x]
                );
            }
        }
    }

    #[test]
    fn test_bilinear_sample_hand_computed() {
        // 3x3 image:
        //   0  1  2
        //   3  4  5
        //   6  7  8
        let data = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let input = Buffer2::new(3, 3, data);

        // At integer pixel (1, 1): exactly 4.0
        assert!(
            (bilinear_sample(&input, Vec2::new(1.0, 1.0), 0.0) - 4.0).abs() < 1e-6,
            "At (1,1): expected 4.0, got {}",
            bilinear_sample(&input, Vec2::new(1.0, 1.0), 0.0)
        );

        // At (0.5, 0.5): bilinear of [0,1,3,4]
        // x0=0, y0=0, fx=0.5, fy=0.5
        // p00=0, p10=1, p01=3, p11=4
        // top = 0 + 0.5*(1-0) = 0.5
        // bottom = 3 + 0.5*(4-3) = 3.5
        // result = 0.5 + 0.5*(3.5 - 0.5) = 0.5 + 1.5 = 2.0
        assert!(
            (bilinear_sample(&input, Vec2::new(0.5, 0.5), 0.0) - 2.0).abs() < 1e-6,
            "At (0.5, 0.5): expected 2.0, got {}",
            bilinear_sample(&input, Vec2::new(0.5, 0.5), 0.0)
        );

        // At (1.5, 0.5): bilinear of [1,2,4,5]
        // x0=1, y0=0, fx=0.5, fy=0.5
        // p00=1, p10=2, p01=4, p11=5
        // top = 1 + 0.5*(2-1) = 1.5
        // bottom = 4 + 0.5*(5-4) = 4.5
        // result = 1.5 + 0.5*(4.5 - 1.5) = 1.5 + 1.5 = 3.0
        assert!(
            (bilinear_sample(&input, Vec2::new(1.5, 0.5), 0.0) - 3.0).abs() < 1e-6,
            "At (1.5, 0.5): expected 3.0, got {}",
            bilinear_sample(&input, Vec2::new(1.5, 0.5), 0.0)
        );

        // At (0.25, 0.75): bilinear of [0,1,3,4]
        // x0=0, y0=0, fx=0.25, fy=0.75
        // top = 0 + 0.25*(1-0) = 0.25
        // bottom = 3 + 0.25*(4-3) = 3.25
        // result = 0.25 + 0.75*(3.25-0.25) = 0.25 + 2.25 = 2.5
        assert!(
            (bilinear_sample(&input, Vec2::new(0.25, 0.75), 0.0) - 2.5).abs() < 1e-6,
            "At (0.25, 0.75): expected 2.5, got {}",
            bilinear_sample(&input, Vec2::new(0.25, 0.75), 0.0)
        );
    }

    #[test]
    fn test_bilinear_sample_border_value() {
        // 2x2 image: [[10, 20], [30, 40]]
        let input = Buffer2::new(2, 2, vec![10.0, 20.0, 30.0, 40.0]);

        // Sampling outside uses border_value
        // At (-1.0, 0.0): x0 = floor(-1.0) = -1, y0 = 0
        // All four neighbors involve x=-1 or x=0
        // p00 = sample(-1, 0) = border = -5.0
        // p10 = sample(0, 0) = 10.0
        // p01 = sample(-1, 1) = border = -5.0
        // p11 = sample(0, 1) = 30.0
        // fx = -1.0 - (-1) = 0.0, fy = 0.0
        // top = -5.0 + 0.0*(10.0 - (-5.0)) = -5.0
        // bottom = -5.0 + 0.0*(30.0 - (-5.0)) = -5.0
        // result = -5.0 + 0.0*(...) = -5.0
        assert!(
            (bilinear_sample(&input, Vec2::new(-1.0, 0.0), -5.0) - (-5.0)).abs() < 1e-6,
            "At (-1.0, 0.0): expected -5.0, got {}",
            bilinear_sample(&input, Vec2::new(-1.0, 0.0), -5.0)
        );
    }

    /// Naive scalar Lanczos row warp for arbitrary `a`, used as reference.
    fn warp_row_lanczos_scalar_ref(
        input: &Buffer2<f32>,
        output_row: &mut [f32],
        output_y: usize,
        wt: &WarpTransform,
        a: usize,
    ) {
        let pixels = input.pixels();
        let input_width = input.width();
        let input_height = input.height();
        let y = output_y as f64;
        let size = 2 * a;
        let a_i32 = a as i32;
        let lut = get_lanczos_lut(a);

        for (x, out_pixel) in output_row.iter_mut().enumerate() {
            let src = wt.apply(DVec2::new(x as f64, y));
            let sx = src.x as f32;
            let sy = src.y as f32;
            let pos = Vec2::new(sx, sy);
            let support_min = -(a as f32);
            if !pos.is_finite()
                || sx < support_min
                || sy < support_min
                || sx >= input_width as f32 + a as f32 - 1.0
                || sy >= input_height as f32 + a as f32 - 1.0
            {
                *out_pixel = 0.0;
                continue;
            }
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            let kx0 = x0 - a_i32 + 1;
            let ky0 = y0 - a_i32 + 1;

            if kx0 < 0
                || ky0 < 0
                || kx0 + size as i32 > input_width as i32
                || ky0 + size as i32 > input_height as i32
            {
                *out_pixel = bilinear_sample_edge_extended(input, pos);
                continue;
            }

            let mut wx = vec![0.0f32; size];
            for (i, w) in wx.iter_mut().enumerate() {
                *w = lut.lookup(fx - (i as i32 - a_i32 + 1) as f32);
            }
            let mut wy = vec![0.0f32; size];
            for (j, w) in wy.iter_mut().enumerate() {
                *w = lut.lookup(fy - (j as i32 - a_i32 + 1) as f32);
            }
            let mut sum = 0.0f32;
            let mut w_in = 0.0f32;
            for (j, &wyj) in wy.iter().enumerate() {
                let py = y0 - a_i32 + 1 + j as i32;
                for (i, &wxi) in wx.iter().enumerate() {
                    let px = x0 - a_i32 + 1 + i as i32;
                    let weight = wxi * wyj;
                    sum += pixels[py as usize * input_width + px as usize] * weight;
                    w_in += weight;
                }
            }
            *out_pixel = if w_in.abs() < 1e-10 { 0.0 } else { sum / w_in };
        }
    }

    #[test]
    fn test_warp_row_lanczos2_matches_scalar_reference() {
        let width = 128;
        let height = 128;
        let input = patterns::diagonal_gradient(width, height);
        let params = WarpParams::new(InterpolationMethod::Lanczos2);

        for transform in [
            Transform::identity(),
            Transform::translation(DVec2::new(2.5, 1.7)),
            Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
        ] {
            let inverse = WarpTransform::new(transform.inverse());
            for y in [0, 50, height - 1] {
                let mut output_fast = vec![0.0f32; width];
                let mut output_scalar = vec![0.0f32; width];
                warp_row_lanczos(&input, &mut output_fast, y, &inverse, &params);
                warp_row_lanczos_scalar_ref(&input, &mut output_scalar, y, &inverse, 2);
                for x in 0..width {
                    assert!(
                        (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                        "L2 row {y}, x={x}: fast {} vs scalar {}",
                        output_fast[x],
                        output_scalar[x]
                    );
                }
            }
        }
    }

    #[test]
    fn test_warp_row_lanczos4_matches_scalar_reference() {
        let width = 128;
        let height = 128;
        let input = patterns::diagonal_gradient(width, height);
        let params = WarpParams::new(InterpolationMethod::Lanczos4);

        for transform in [
            Transform::identity(),
            Transform::translation(DVec2::new(2.5, 1.7)),
            Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
        ] {
            let inverse = WarpTransform::new(transform.inverse());
            for y in [0, 50, height - 1] {
                let mut output_fast = vec![0.0f32; width];
                let mut output_scalar = vec![0.0f32; width];
                warp_row_lanczos(&input, &mut output_fast, y, &inverse, &params);
                warp_row_lanczos_scalar_ref(&input, &mut output_scalar, y, &inverse, 4);
                for x in 0..width {
                    assert!(
                        (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                        "L4 row {y}, x={x}: fast {} vs scalar {}",
                        output_fast[x],
                        output_scalar[x]
                    );
                }
            }
        }
    }

    #[test]
    fn test_warp_row_lanczos2_various_sizes() {
        let height = 64;
        let input_base = patterns::diagonal_gradient(256, height);
        let params = WarpParams::new(InterpolationMethod::Lanczos2);

        for width in [1, 2, 3, 7, 8, 16, 33, 64, 100] {
            let input = Buffer2::new(
                width,
                height,
                input_base
                    .pixels()
                    .iter()
                    .take(width * height)
                    .copied()
                    .collect(),
            );
            let transform = Transform::translation(DVec2::new(1.5, 0.5));
            let inverse = WarpTransform::new(transform.inverse());

            let mut output_fast = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            let y = height / 2;

            warp_row_lanczos(&input, &mut output_fast, y, &inverse, &params);
            warp_row_lanczos_scalar_ref(&input, &mut output_scalar, y, &inverse, 2);

            for x in 0..width {
                assert!(
                    (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                    "L2 width {width}, x={x}: fast {} vs scalar {}",
                    output_fast[x],
                    output_scalar[x]
                );
            }
        }
    }

    #[test]
    fn test_warp_row_lanczos4_various_sizes() {
        let height = 64;
        let input_base = patterns::diagonal_gradient(256, height);
        let params = WarpParams::new(InterpolationMethod::Lanczos4);

        for width in [1, 2, 3, 7, 8, 16, 33, 64, 100] {
            let input = Buffer2::new(
                width,
                height,
                input_base
                    .pixels()
                    .iter()
                    .take(width * height)
                    .copied()
                    .collect(),
            );
            let transform = Transform::translation(DVec2::new(1.5, 0.5));
            let inverse = WarpTransform::new(transform.inverse());

            let mut output_fast = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            let y = height / 2;

            warp_row_lanczos(&input, &mut output_fast, y, &inverse, &params);
            warp_row_lanczos_scalar_ref(&input, &mut output_scalar, y, &inverse, 4);

            for x in 0..width {
                assert!(
                    (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                    "L4 width {width}, x={x}: fast {} vs scalar {}",
                    output_fast[x],
                    output_scalar[x]
                );
            }
        }
    }
}
