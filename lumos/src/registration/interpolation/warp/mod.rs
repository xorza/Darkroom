//! Optimized row-warping implementations.
//!
//! Runtime dispatch to the best available implementation:
//! - **Bilinear**: AVX2/SSE4.1 on x86_64, scalar with incremental stepping elsewhere
//! - **Lanczos3**: Scalar with incremental stepping, fast-path interior bounds skipping

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

use crate::registration::interpolation::get_lanczos_lut;
use crate::registration::transform::Transform;
use glam::DVec2;

/// Warp a row of pixels using bilinear interpolation.
///
/// Uses AVX2/SSE4.1 on x86_64, scalar with incremental stepping elsewhere.
#[inline]
pub(crate) fn warp_row_bilinear(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &Transform,
) {
    #[cfg(target_arch = "x86_64")]
    {
        let output_width = output_row.len();
        if output_width >= 8 && cpu_features::has_avx2() {
            unsafe {
                sse::warp_row_bilinear_avx2(
                    input,
                    input_width,
                    input_height,
                    output_row,
                    output_y,
                    inverse,
                );
            }
            return;
        }
        if output_width >= 4 && cpu_features::has_sse4_1() {
            unsafe {
                sse::warp_row_bilinear_sse(
                    input,
                    input_width,
                    input_height,
                    output_row,
                    output_y,
                    inverse,
                );
            }
            return;
        }
    }

    // Scalar fallback (also used on aarch64 - NEON was slower due to gather overhead)
    warp_row_bilinear_scalar(
        input,
        input_width,
        input_height,
        output_row,
        output_y,
        inverse,
    );
}

/// Scalar implementation of row warping with bilinear interpolation.
///
/// Uses incremental coordinate stepping for affine transforms to avoid
/// per-pixel matrix multiply.
pub(crate) fn warp_row_bilinear_scalar(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &Transform,
) {
    let m = inverse.matrix.as_array();
    let is_affine = m[6].abs() < 1e-12 && m[7].abs() < 1e-12;

    let src0 = inverse.apply(DVec2::new(0.0, output_y as f64));
    let mut src_x = src0.x;
    let mut src_y = src0.y;
    let dx_step = m[0];
    let dy_step = m[3];

    for (x_idx, out_pixel) in output_row.iter_mut().enumerate() {
        if !is_affine {
            let src = inverse.apply(DVec2::new(x_idx as f64, output_y as f64));
            src_x = src.x;
            src_y = src.y;
        }

        *out_pixel = bilinear_sample(input, input_width, input_height, src_x as f32, src_y as f32);

        if is_affine {
            src_x += dx_step;
            src_y += dy_step;
        }
    }
}

use super::sample_pixel;

/// Bilinear sampling at a single point (f32 coordinates).
#[inline]
pub(crate) fn bilinear_sample(input: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = sample_pixel(input, width, height, x0, y0);
    let p10 = sample_pixel(input, width, height, x0 + 1, y0);
    let p01 = sample_pixel(input, width, height, x0, y0 + 1);
    let p11 = sample_pixel(input, width, height, x0 + 1, y0 + 1);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);
    top + fy * (bottom - top)
}

/// Optimized Lanczos3 row warping with incremental coordinate stepping.
///
/// Key optimizations over per-pixel `interpolate_lanczos_impl`:
/// 1. Incremental source coordinate stepping (avoid per-pixel matrix multiply)
/// 2. Fast-path for interior pixels (skip bounds checks, use direct row pointers)
/// 3. Row pointer caching (compute `y * width` once per kernel row, not 6 times)
pub(crate) fn warp_row_lanczos3(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &Transform,
) {
    let lut = get_lanczos_lut(3);
    let iw = input_width as i32;
    let ih = input_height as i32;

    // Row-major DMat3: m[0..9] = [a, b, tx, c, d, ty, g, h, 1]
    // transform_point: x' = (m[0]*x + m[1]*y + m[2]) / w, y' = (m[3]*x + m[4]*y + m[5]) / w
    // For affine transforms (m[6]==0, m[7]==0, m[8]==1), stepping x by 1 adds (m[0], m[3]).
    let m = inverse.matrix.as_array();
    let is_affine = m[6].abs() < 1e-12 && m[7].abs() < 1e-12;

    // Compute source coords for first pixel in this row
    let src0 = inverse.apply(DVec2::new(0.0, output_y as f64));
    let mut src_x = src0.x;
    let mut src_y = src0.y;
    let dx_step = m[0];
    let dy_step = m[3];

    for (x_idx, out_pixel) in output_row.iter_mut().enumerate() {
        // For perspective transforms, recompute exact coords each pixel
        if !is_affine {
            let src = inverse.apply(DVec2::new(x_idx as f64, output_y as f64));
            src_x = src.x;
            src_y = src.y;
        }

        let sx = src_x as f32;
        let sy = src_y as f32;

        let x0 = sx.floor() as i32;
        let y0 = sy.floor() as i32;
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;

        // Kernel origin: (x0 - 2, y0 - 2)
        let kx0 = x0 - 2;
        let ky0 = y0 - 2;

        // Compute x weights (6 values)
        let wx = [
            lut.lookup(fx + 2.0),
            lut.lookup(fx + 1.0),
            lut.lookup(fx),
            lut.lookup(fx - 1.0),
            lut.lookup(fx - 2.0),
            lut.lookup(fx - 3.0),
        ];

        // Compute y weights (6 values)
        let wy = [
            lut.lookup(fy + 2.0),
            lut.lookup(fy + 1.0),
            lut.lookup(fy),
            lut.lookup(fy - 1.0),
            lut.lookup(fy - 2.0),
            lut.lookup(fy - 3.0),
        ];

        // Normalize weights
        let wx_sum: f32 = wx[0] + wx[1] + wx[2] + wx[3] + wx[4] + wx[5];
        let wy_sum: f32 = wy[0] + wy[1] + wy[2] + wy[3] + wy[4] + wy[5];
        let inv_wx = if wx_sum.abs() > 1e-10 {
            1.0 / wx_sum
        } else {
            1.0
        };
        let inv_wy = if wy_sum.abs() > 1e-10 {
            1.0 / wy_sum
        } else {
            1.0
        };

        // Normalized x weights
        let nwx = [
            wx[0] * inv_wx,
            wx[1] * inv_wx,
            wx[2] * inv_wx,
            wx[3] * inv_wx,
            wx[4] * inv_wx,
            wx[5] * inv_wx,
        ];

        // Check if entire 6x6 kernel is within bounds
        let sum = if kx0 >= 0 && ky0 >= 0 && kx0 + 5 < iw && ky0 + 5 < ih {
            // Fast path: all pixels in bounds, direct indexing
            let kx = kx0 as usize;
            let ky = ky0 as usize;
            let w = input_width;

            let mut sum = 0.0f32;
            for (j, &wyj_raw) in wy.iter().enumerate() {
                let row_off = (ky + j) * w + kx;
                let wyj = wyj_raw * inv_wy;
                sum += wyj
                    * (unsafe { *input.get_unchecked(row_off) } * nwx[0]
                        + unsafe { *input.get_unchecked(row_off + 1) } * nwx[1]
                        + unsafe { *input.get_unchecked(row_off + 2) } * nwx[2]
                        + unsafe { *input.get_unchecked(row_off + 3) } * nwx[3]
                        + unsafe { *input.get_unchecked(row_off + 4) } * nwx[4]
                        + unsafe { *input.get_unchecked(row_off + 5) } * nwx[5]);
            }
            sum
        } else {
            // Slow path: bounds-checked sampling for border pixels
            let mut sum = 0.0f32;
            for (j, &wyj_raw) in wy.iter().enumerate() {
                let py = ky0 + j as i32;
                let wyj = wyj_raw * inv_wy;
                for (i, &wxi) in nwx.iter().enumerate() {
                    let px = kx0 + i as i32;
                    sum += sample_pixel(input, input_width, input_height, px, py) * wxi * wyj;
                }
            }
            sum
        };

        *out_pixel = sum;

        if is_affine {
            src_x += dx_step;
            src_y += dy_step;
        }
    }
}

/// Scalar implementation of row warping with Lanczos3 interpolation.
#[cfg(test)]
pub fn warp_row_lanczos3_scalar(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &Transform,
) {
    let y = output_y as f64;
    const A: usize = 3; // Lanczos3 kernel radius

    for (x, out_pixel) in output_row.iter_mut().enumerate() {
        let src = inverse.apply(DVec2::new(x as f64, y));
        let sx = src.x as f32;
        let sy = src.y as f32;

        let x0 = sx.floor() as i32;
        let y0 = sy.floor() as i32;
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;

        let lut = get_lanczos_lut(A);

        // Pre-compute x weights (6 values for Lanczos3)
        let mut wx = [0.0f32; 6];
        for (i, w) in wx.iter_mut().enumerate() {
            let dx = fx - (i as i32 - 2) as f32;
            *w = lut.lookup(dx);
        }

        // Pre-compute y weights
        let mut wy = [0.0f32; 6];
        for (j, w) in wy.iter_mut().enumerate() {
            let dy = fy - (j as i32 - 2) as f32;
            *w = lut.lookup(dy);
        }

        // Normalize weights to preserve brightness
        let wx_sum: f32 = wx.iter().sum();
        let wy_sum: f32 = wy.iter().sum();
        if wx_sum.abs() > 1e-10 {
            wx.iter_mut().for_each(|w| *w /= wx_sum);
        }
        if wy_sum.abs() > 1e-10 {
            wy.iter_mut().for_each(|w| *w /= wy_sum);
        }

        // Compute weighted sum
        let mut sum = 0.0f32;

        for (j, &wyj) in wy.iter().enumerate() {
            let py = y0 - 2 + j as i32;
            for (i, &wxi) in wx.iter().enumerate() {
                let px = x0 - 2 + i as i32;
                let pixel = sample_pixel(input, input_width, input_height, px, py);
                sum += pixel * wxi * wyj;
            }
        }

        *out_pixel = sum;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::synthetic::patterns;

    #[test]
    fn test_warp_row_bilinear_identity() {
        let width = 100;
        let height = 100;
        let input = patterns::diagonal_gradient(width, height);
        let identity = Transform::identity();

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_bilinear(input.pixels(), width, height, &mut output_row, y, &identity);

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
        let inverse = transform.inverse();

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_bilinear(input.pixels(), width, height, &mut output_row, y, &inverse);

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
            let inverse = transform.inverse();

            for y in [0, 50, height - 1] {
                let mut output_simd = vec![0.0f32; width];
                let mut output_scalar = vec![0.0f32; width];

                warp_row_bilinear(input.pixels(), width, height, &mut output_simd, y, &inverse);

                warp_row_bilinear_scalar(
                    input.pixels(),
                    width,
                    height,
                    &mut output_scalar,
                    y,
                    &inverse,
                );

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
            let input: Vec<f32> = input_base
                .pixels()
                .iter()
                .take(width * height)
                .copied()
                .collect();
            let identity = Transform::identity();

            let mut output_simd = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            let y = height / 2;

            warp_row_bilinear(&input, width, height, &mut output_simd, y, &identity);
            warp_row_bilinear_scalar(&input, width, height, &mut output_scalar, y, &identity);

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
        let identity = Transform::identity();

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_lanczos3_scalar(input.pixels(), width, height, &mut output_row, y, &identity);

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
            let input: Vec<f32> = input_base
                .pixels()
                .iter()
                .take(width * height)
                .copied()
                .collect();
            let transform = Transform::translation(DVec2::new(1.5, 0.5));
            let inverse = transform.inverse();

            let mut output = vec![0.0f32; width];
            let y = height / 2;

            warp_row_lanczos3_scalar(&input, width, height, &mut output, y, &inverse);

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

        let transforms = vec![
            Transform::identity(),
            Transform::translation(DVec2::new(2.5, 1.7)),
            Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
        ];

        for transform in transforms {
            let inverse = transform.inverse();

            for y in [0, 50, height - 1] {
                let mut output_fast = vec![0.0f32; width];
                let mut output_scalar = vec![0.0f32; width];

                warp_row_lanczos3(input.pixels(), width, height, &mut output_fast, y, &inverse);
                warp_row_lanczos3_scalar(
                    input.pixels(),
                    width,
                    height,
                    &mut output_scalar,
                    y,
                    &inverse,
                );

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

        for width in [1, 2, 3, 7, 8, 16, 33, 64, 100] {
            let input: Vec<f32> = input_base
                .pixels()
                .iter()
                .take(width * height)
                .copied()
                .collect();
            let transform = Transform::translation(DVec2::new(1.5, 0.5));
            let inverse = transform.inverse();

            let mut output_fast = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            let y = height / 2;

            warp_row_lanczos3(&input, width, height, &mut output_fast, y, &inverse);
            warp_row_lanczos3_scalar(&input, width, height, &mut output_scalar, y, &inverse);

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
}
