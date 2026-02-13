//! Optimized row-warping implementations.
//!
//! Runtime dispatch to the best available implementation:
//! - **Bilinear**: AVX2/SSE4.1 on x86_64, scalar with incremental stepping elsewhere
//! - **Lanczos3**: Scalar with incremental stepping, fast-path interior bounds skipping
//!
//! When SIP distortion correction is active, incremental stepping is disabled
//! (SIP is nonlinear) and SIMD paths fall back to scalar.

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

use crate::common::Buffer2;
use crate::registration::interpolation::WarpParams;
use crate::registration::interpolation::get_lanczos_lut;
use crate::registration::transform::WarpTransform;
use glam::DVec2;

/// Warp a row of pixels using bilinear interpolation.
///
/// Uses AVX2/SSE4.1 on x86_64, scalar with incremental stepping elsewhere.
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

    // Scalar fallback (also used on aarch64 and when SIP is active)
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
    let m = wt.transform.matrix.as_array();
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

        *out_pixel = bilinear_sample(input, src_x as f32, src_y as f32, border_value);

        if can_step {
            src_x += dx_step;
            src_y += dy_step;
        }
    }
}

use super::sample_pixel;

/// Bilinear sampling at a single point (f32 coordinates).
#[inline]
pub(crate) fn bilinear_sample(input: &Buffer2<f32>, x: f32, y: f32, border_value: f32) -> f32 {
    let pixels = input.pixels();
    let width = input.width();
    let height = input.height();

    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = sample_pixel(pixels, width, height, x0, y0, border_value);
    let p10 = sample_pixel(pixels, width, height, x0 + 1, y0, border_value);
    let p01 = sample_pixel(pixels, width, height, x0, y0 + 1, border_value);
    let p11 = sample_pixel(pixels, width, height, x0 + 1, y0 + 1, border_value);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);
    top + fy * (bottom - top)
}

/// PixInsight-style soft clamping for Lanczos deringing.
///
/// Tracks positive (`sp`) and negative (`sn`) weighted contributions separately.
/// When the ratio `sn/sp` exceeds the threshold, a quadratic fade reduces the
/// negative contribution, preserving sharpness while suppressing ringing.
///
/// Reference: PCL LanczosInterpolation.h (BSD license)
#[inline]
fn soft_clamp(sp: f32, sn: f32, wp: f32, wn: f32, th: f32, th_inv: f32) -> f32 {
    if sp == 0.0 {
        return 0.0;
    }
    let r = sn / sp;
    if r >= 1.0 {
        return sp / wp;
    }
    if r > th {
        let fade = (r - th) * th_inv;
        let c = 1.0 - fade * fade;
        return (sp - sn * c) / (wp - wn * c);
    }
    (sp - sn) / (wp - wn)
}

/// Optimized Lanczos3 row warping with incremental coordinate stepping.
///
/// Key optimizations:
/// 1. Const-generic DERINGING: eliminates inner-loop branch, letting LLVM fully vectorize
/// 2. PixInsight-style soft clamping (positive/negative contribution tracking)
/// 3. Incremental source coordinate stepping (avoid per-pixel matrix multiply)
/// 4. Fast-path for interior pixels (skip bounds checks, use direct row pointers)
pub(crate) fn warp_row_lanczos3(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    params: &WarpParams,
) {
    if params.method.deringing_enabled() {
        warp_row_lanczos3_inner::<true>(input, output_row, output_y, wt, params);
    } else {
        warp_row_lanczos3_inner::<false>(input, output_row, output_y, wt, params);
    }
}

fn warp_row_lanczos3_inner<const DERINGING: bool>(
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

    let lut = get_lanczos_lut(3);
    let iw = input_width as i32;
    let ih = input_height as i32;

    #[cfg(target_arch = "x86_64")]
    let use_fma = cpu_features::has_avx2_fma();

    // Pre-compute clamping threshold parameters (only used when DERINGING=true)
    let clamp_th = params.method.deringing().clamp(0.0, 1.0);
    let clamp_th_inv = if clamp_th < 1.0 {
        1.0 / (1.0 - clamp_th)
    } else {
        0.0
    };

    let m = wt.transform.matrix.as_array();
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

        let sx = src_x as f32;
        let sy = src_y as f32;

        let x0 = sx.floor() as i32;
        let y0 = sy.floor() as i32;
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;

        let kx0 = x0 - 2;
        let ky0 = y0 - 2;

        let wx = [
            lut.lookup(fx + 2.0),
            lut.lookup(fx + 1.0),
            lut.lookup(fx),
            lut.lookup(fx - 1.0),
            lut.lookup(fx - 2.0),
            lut.lookup(fx - 3.0),
        ];
        let wy = [
            lut.lookup(fy + 2.0),
            lut.lookup(fy + 1.0),
            lut.lookup(fy),
            lut.lookup(fy - 1.0),
            lut.lookup(fy - 2.0),
            lut.lookup(fy - 3.0),
        ];

        let wx_sum = wx[0] + wx[1] + wx[2] + wx[3] + wx[4] + wx[5];
        let wy_sum = wy[0] + wy[1] + wy[2] + wy[3] + wy[4] + wy[5];
        let total_sum = wx_sum * wy_sum;
        let inv_total = if total_sum.abs() > 1e-10 {
            1.0 / total_sum
        } else {
            1.0
        };

        // SIMD fast path: needs kx0+7 < iw for 128-bit loads (6 valid + 2 padding)
        #[cfg(target_arch = "x86_64")]
        if kx0 >= 0 && ky0 >= 0 && kx0 + 8 < iw && ky0 + 5 < ih && use_fma {
            let (sp, sn, wp, wn) = unsafe {
                sse::lanczos3_kernel_fma::<DERINGING>(
                    pixels,
                    input_width,
                    kx0 as usize,
                    ky0 as usize,
                    &wx,
                    &wy,
                )
            };
            *out_pixel = if DERINGING {
                soft_clamp(sp, sn, wp, wn, clamp_th, clamp_th_inv)
            } else {
                sp * inv_total
            };

            if can_step {
                src_x += dx_step;
                src_y += dy_step;
            }
            continue;
        }

        if kx0 >= 0 && ky0 >= 0 && kx0 + 5 < iw && ky0 + 5 < ih {
            // Scalar fast path: all 6x6 pixels in bounds, direct indexing
            let kx = kx0 as usize;
            let ky = ky0 as usize;
            let w = input_width;

            let mut sp = 0.0f32;
            let mut sn = 0.0f32;
            let mut wp = 0.0f32;
            let mut wn = 0.0f32;
            for (j, &wyj) in wy.iter().enumerate() {
                let row_off = (ky + j) * w + kx;
                for (k, &wxk) in wx.iter().enumerate() {
                    let v = unsafe { *pixels.get_unchecked(row_off + k) };
                    let weight = wxk * wyj;
                    let s = v * weight;
                    if DERINGING {
                        if s < 0.0 {
                            sn -= s;
                            wn -= weight;
                        } else {
                            sp += s;
                            wp += weight;
                        }
                    } else {
                        sp += s;
                    }
                }
            }
            *out_pixel = if DERINGING {
                soft_clamp(sp, sn, wp, wn, clamp_th, clamp_th_inv)
            } else {
                sp * inv_total
            };
        } else {
            // Slow path: bounds-checked sampling for border pixels
            let mut sp = 0.0f32;
            let mut sn = 0.0f32;
            let mut wp = 0.0f32;
            let mut wn = 0.0f32;
            for (j, &wyj) in wy.iter().enumerate() {
                let py = ky0 + j as i32;
                for (i, &wxi) in wx.iter().enumerate() {
                    let px = kx0 + i as i32;
                    let v = sample_pixel(pixels, input_width, input_height, px, py, border_value);
                    let weight = wxi * wyj;
                    let s = v * weight;
                    if DERINGING {
                        if s < 0.0 {
                            sn -= s;
                            wn -= weight;
                        } else {
                            sp += s;
                            wp += weight;
                        }
                    } else {
                        sp += s;
                    }
                }
            }
            *out_pixel = if DERINGING {
                soft_clamp(sp, sn, wp, wn, clamp_th, clamp_th_inv)
            } else {
                sp * inv_total
            };
        }

        if can_step {
            src_x += dx_step;
            src_y += dy_step;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registration::config::InterpolationMethod;
    use crate::registration::interpolation::WarpParams;
    use crate::registration::transform::Transform;
    use crate::testing::synthetic::patterns;

    /// Naive scalar Lanczos3 row warp used as reference for testing the optimized version.
    fn warp_row_lanczos3_scalar(
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
            let sx = src.x as f32;
            let sy = src.y as f32;

            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;

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

            let wx_sum: f32 = wx.iter().sum();
            let wy_sum: f32 = wy.iter().sum();
            if wx_sum.abs() > 1e-10 {
                wx.iter_mut().for_each(|w| *w /= wx_sum);
            }
            if wy_sum.abs() > 1e-10 {
                wy.iter_mut().for_each(|w| *w /= wy_sum);
            }

            let mut sum = 0.0f32;
            for (j, &wyj) in wy.iter().enumerate() {
                let py = y0 - 2 + j as i32;
                for (i, &wxi) in wx.iter().enumerate() {
                    let px = x0 - 2 + i as i32;
                    let pixel = sample_pixel(pixels, input_width, input_height, px, py, 0.0);
                    sum += pixel * wxi * wyj;
                }
            }

            *out_pixel = sum;
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
    fn test_warp_row_lanczos3_scalar_identity() {
        let width = 100;
        let height = 100;
        let input = patterns::diagonal_gradient(width, height);
        let identity = WarpTransform::new(Transform::identity());

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_lanczos3_scalar(&input, &mut output_row, y, &identity);

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
    fn test_warp_row_lanczos3_scalar_various_sizes() {
        let height = 64;
        let input_base = patterns::diagonal_gradient(256, height);

        // Test various widths
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

            let mut output = vec![0.0f32; width];
            let y = height / 2;

            warp_row_lanczos3_scalar(&input, &mut output, y, &inverse);

            // Just verify no panics and output is reasonable
            for (x, &val) in output
                .iter()
                .enumerate()
                .take(width.saturating_sub(3))
                .skip(3)
            {
                assert!(
                    val.is_finite(),
                    "Width {}, x={}: output is not finite: {}",
                    width,
                    x,
                    val
                );
            }
        }
    }

    #[test]
    fn test_warp_row_lanczos3_matches_scalar() {
        let width = 128;
        let height = 128;
        let input = patterns::diagonal_gradient(width, height);
        // Disable clamping to match unclamped scalar reference
        let params = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: -1.0 });

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

                warp_row_lanczos3(&input, &mut output_fast, y, &inverse, &params);
                warp_row_lanczos3_scalar(&input, &mut output_scalar, y, &inverse);

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
    fn test_warp_row_lanczos3_various_sizes() {
        let height = 64;
        let input_base = patterns::diagonal_gradient(256, height);
        // Disable clamping to match unclamped scalar reference
        let params = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: -1.0 });

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

            warp_row_lanczos3(&input, &mut output_fast, y, &inverse, &params);
            warp_row_lanczos3_scalar(&input, &mut output_scalar, y, &inverse);

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

    // --- Soft deringing tests ---

    #[test]
    fn test_soft_clamp_pure_positive() {
        let result = soft_clamp(2.0, 0.0, 1.0, 0.0, 0.3, 1.0 / 0.7);
        assert!((result - 2.0).abs() < 1e-6, "got {result}");
    }

    #[test]
    fn test_soft_clamp_all_zero() {
        assert_eq!(soft_clamp(0.0, 0.0, 0.0, 0.0, 0.3, 1.0 / 0.7), 0.0);
    }

    #[test]
    fn test_soft_clamp_below_threshold() {
        // r = 0.1/1.0 = 0.1 < 0.3 → no fade
        let result = soft_clamp(1.0, 0.1, 0.8, 0.15, 0.3, 1.0 / 0.7);
        let expected = (1.0 - 0.1) / (0.8 - 0.15);
        assert!(
            (result - expected).abs() < 1e-6,
            "got {result}, expected {expected}"
        );
    }

    #[test]
    fn test_soft_clamp_above_threshold() {
        let th = 0.3f32;
        let th_inv = 1.0 / (1.0 - th);
        let sp = 1.0f32;
        let sn = 0.5f32;
        let wp = 0.8f32;
        let wn = 0.3f32;

        let fade = (sn / sp - th) * th_inv;
        let c = 1.0 - fade * fade;
        let expected = (sp - sn * c) / (wp - wn * c);

        let result = soft_clamp(sp, sn, wp, wn, th, th_inv);
        assert!(
            (result - expected).abs() < 1e-6,
            "got {result}, expected {expected}"
        );
    }

    #[test]
    fn test_soft_clamp_ratio_at_one() {
        let result = soft_clamp(1.0, 1.0, 0.8, 0.3, 0.3, 1.0 / 0.7);
        assert!((result - 1.0 / 0.8).abs() < 1e-6, "got {result}");
    }

    #[test]
    fn test_soft_clamp_ratio_above_one() {
        let result = soft_clamp(1.0, 1.5, 0.8, 0.5, 0.3, 1.0 / 0.7);
        assert!((result - 1.0 / 0.8).abs() < 1e-6, "got {result}");
    }

    #[test]
    fn test_soft_clamp_threshold_zero() {
        let th = 0.0f32;
        let th_inv = 1.0f32;
        let fade = 0.01 * 1.0;
        let c = 1.0 - fade * fade;
        let expected = (1.0 - 0.01 * c) / (0.9 - 0.05 * c);
        let result = soft_clamp(1.0, 0.01, 0.9, 0.05, th, th_inv);
        assert!(
            (result - expected).abs() < 1e-6,
            "got {result}, expected {expected}"
        );
    }

    #[test]
    fn test_soft_clamp_threshold_one() {
        let expected = (1.0f32 - 0.5) / (0.8f32 - 0.3);
        let result = soft_clamp(1.0, 0.5, 0.8, 0.3, 1.0, 0.0);
        assert!(
            (result - expected).abs() < 1e-6,
            "got {result}, expected {expected}"
        );
    }

    #[test]
    fn test_soft_clamp_result_bounded() {
        // soft_clamp output should always be >= 0 for non-negative sp,
        // and <= sp/wp (the "only positive" baseline)
        let th = 0.3f32;
        let th_inv = 1.0 / (1.0 - th);
        let sp = 1.0f32;
        let wp = 0.9f32;
        let positive_only = sp / wp;

        for i in 0..20 {
            let sn = i as f32 * 0.05;
            let wn = sn * 0.5;
            let result = soft_clamp(sp, sn, wp, wn, th, th_inv);
            assert!(result >= 0.0, "negative output at sn={sn}: {result}");
            assert!(
                result <= positive_only + 1e-6,
                "exceeds positive-only baseline at sn={sn}: {result} > {positive_only}"
            );
        }
    }

    /// Naive scalar Lanczos3 with deringing for reference testing.
    fn warp_row_lanczos3_scalar_deringing(
        input: &Buffer2<f32>,
        output_row: &mut [f32],
        output_y: usize,
        wt: &WarpTransform,
        th: f32,
        th_inv: f32,
    ) {
        let pixels = input.pixels();
        let input_width = input.width();
        let input_height = input.height();
        let y = output_y as f64;
        let lut = get_lanczos_lut(3);

        for (x, out_pixel) in output_row.iter_mut().enumerate() {
            let src = wt.apply(DVec2::new(x as f64, y));
            let sx = src.x as f32;
            let sy = src.y as f32;
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;

            let mut wx = [0.0f32; 6];
            for (i, w) in wx.iter_mut().enumerate() {
                *w = lut.lookup(fx - (i as i32 - 2) as f32);
            }
            let mut wy = [0.0f32; 6];
            for (j, w) in wy.iter_mut().enumerate() {
                *w = lut.lookup(fy - (j as i32 - 2) as f32);
            }

            let wx_sum: f32 = wx.iter().sum();
            let wy_sum: f32 = wy.iter().sum();
            if wx_sum.abs() > 1e-10 {
                wx.iter_mut().for_each(|w| *w /= wx_sum);
            }
            if wy_sum.abs() > 1e-10 {
                wy.iter_mut().for_each(|w| *w /= wy_sum);
            }

            let mut sp = 0.0f32;
            let mut sn = 0.0f32;
            let mut wp = 0.0f32;
            let mut wn = 0.0f32;
            for (j, &wyj) in wy.iter().enumerate() {
                let py = y0 - 2 + j as i32;
                for (i, &wxi) in wx.iter().enumerate() {
                    let px = x0 - 2 + i as i32;
                    let v = sample_pixel(pixels, input_width, input_height, px, py, 0.0);
                    let weight = wxi * wyj;
                    let s = v * weight;
                    if s < 0.0 {
                        sn -= s;
                        wn -= weight;
                    } else {
                        sp += s;
                        wp += weight;
                    }
                }
            }
            *out_pixel = soft_clamp(sp, sn, wp, wn, th, th_inv);
        }
    }

    #[test]
    fn test_warp_row_lanczos3_deringing_simd_vs_scalar() {
        let width = 128;
        let height = 128;
        let input = patterns::checkerboard(width, height, 8, 0.0, 1.0);
        let params = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 });
        let th = 0.3f32;
        let th_inv = 1.0 / (1.0 - th);

        for transform in [
            Transform::translation(DVec2::new(2.5, 1.7)),
            Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
        ] {
            let inverse = WarpTransform::new(transform.inverse());
            for y in [20, 50, 80] {
                let mut output_fast = vec![0.0f32; width];
                let mut output_scalar = vec![0.0f32; width];
                warp_row_lanczos3(&input, &mut output_fast, y, &inverse, &params);
                warp_row_lanczos3_scalar_deringing(
                    &input,
                    &mut output_scalar,
                    y,
                    &inverse,
                    th,
                    th_inv,
                );
                for x in 0..width {
                    assert!(
                        (output_fast[x] - output_scalar[x]).abs() < 1e-3,
                        "y={y}, x={x}: fast {} vs scalar {}",
                        output_fast[x],
                        output_scalar[x]
                    );
                }
            }
        }
    }

    #[test]
    fn test_deringing_suppresses_ringing_on_bright_star() {
        let size = 32;
        let mut input = patterns::uniform(size, size, 0.0);
        input[(size / 2, size / 2)] = 1.0;

        let transform = Transform::translation(DVec2::new(0.3, 0.3));
        let inverse = WarpTransform::new(transform.inverse());

        // Without deringing: expect negative ringing
        let params_off = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: -1.0 });
        let mut row_off = vec![0.0f32; size];
        warp_row_lanczos3(&input, &mut row_off, size / 2, &inverse, &params_off);
        let min_off = row_off.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(min_off < -0.001, "Expected negative ringing, min={min_off}");

        // With deringing: negative values should be suppressed
        let params_on = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 });
        let mut row_on = vec![0.0f32; size];
        warp_row_lanczos3(&input, &mut row_on, size / 2, &inverse, &params_on);
        let min_on = row_on.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            min_on > min_off,
            "Deringing should help: {min_on} vs {min_off}"
        );
        assert!(min_on >= -0.01, "Should suppress negatives, got {min_on}");
    }

    #[test]
    fn test_deringing_preserves_smooth_gradient() {
        let width = 128;
        let height = 64;
        let input = patterns::horizontal_gradient(width, height, 0.2, 0.8);
        let transform = Transform::translation(DVec2::new(0.5, 0.5));
        let inverse = WarpTransform::new(transform.inverse());

        let params_on = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 });
        let params_off = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: -1.0 });
        let y = height / 2;
        let mut row_on = vec![0.0f32; width];
        let mut row_off = vec![0.0f32; width];
        warp_row_lanczos3(&input, &mut row_on, y, &inverse, &params_on);
        warp_row_lanczos3(&input, &mut row_off, y, &inverse, &params_off);

        // Smooth gradient: deringing should produce nearly identical results
        let max_diff = row_on[5..width - 5]
            .iter()
            .zip(&row_off[5..width - 5])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_diff < 0.005, "max diff = {max_diff}");

        // Monotonicity preserved
        for x in 6..width - 5 {
            assert!(
                row_on[x] >= row_on[x - 1] - 0.01,
                "Monotonicity broken at x={x}"
            );
        }
    }
}
