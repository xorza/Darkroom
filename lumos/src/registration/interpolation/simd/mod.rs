//! SIMD-accelerated interpolation utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE4.1 on x86_64
//! - Scalar fallback on aarch64 (NEON removed - benchmarks showed scalar is faster)
//!
//! # Supported Interpolation Methods
//!
//! - **Bilinear**: SIMD-accelerated on x86_64 (8 pixels/cycle on AVX2)
//! - **Lanczos3**: Scalar only - AVX2 tested but provides no benefit due to memory-bound
//!   random access patterns from arbitrary geometric transforms

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(test)]
use crate::registration::interpolation::get_lanczos_lut;
use crate::registration::transform::Transform;
use glam::DVec2;

/// Warp a row of pixels using SIMD-accelerated bilinear interpolation.
///
/// Uses AVX2/SSE4.1 on x86_64, scalar on other platforms.
///
/// # Arguments
/// * `input` - Input image data (row-major)
/// * `input_width` - Width of input image
/// * `input_height` - Height of input image
/// * `output_row` - Output buffer for this row
/// * `output_y` - Y coordinate of this row
/// * `inverse` - Inverse transform (output -> input)
#[inline]
pub(crate) fn warp_row_bilinear_simd(
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
pub(crate) fn warp_row_bilinear_scalar(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &Transform,
) {
    let y = output_y as f64;

    for (x, out_pixel) in output_row.iter_mut().enumerate() {
        let src = inverse.apply(DVec2::new(x as f64, y));

        *out_pixel = bilinear_sample(input, input_width, input_height, src.x as f32, src.y as f32);
    }
}

/// Bilinear sampling at a single point (f32 coordinates).
///
/// This is the SIMD-compatible implementation using f32 coordinates for fast warping.
#[inline]
pub(crate) fn bilinear_sample(input: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = sample_pixel(input, width, height, x0, y0);
    let p10 = sample_pixel(input, width, height, x1, y0);
    let p01 = sample_pixel(input, width, height, x0, y1);
    let p11 = sample_pixel(input, width, height, x1, y1);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);

    top + fy * (bottom - top)
}

/// Sample a pixel with bounds checking.
#[inline]
fn sample_pixel(data: &[f32], width: usize, height: usize, x: i32, y: i32) -> f32 {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        0.0
    } else {
        data[y as usize * width + x as usize]
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

/// Warp an entire image using Lanczos3 interpolation.
#[cfg(test)]
pub fn warp_image_lanczos3(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    transform: &Transform,
) -> Vec<f32> {
    let mut output = vec![0.0; output_width * output_height];
    let inverse = transform.inverse();

    for y in 0..output_height {
        let row_start = y * output_width;
        let row_end = row_start + output_width;
        warp_row_lanczos3_scalar(
            input,
            input_width,
            input_height,
            &mut output[row_start..row_end],
            y,
            &inverse,
        );
    }

    output
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

        warp_row_bilinear_simd(input.pixels(), width, height, &mut output_row, y, &identity);

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

        warp_row_bilinear_simd(input.pixels(), width, height, &mut output_row, y, &inverse);

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

                warp_row_bilinear_simd(
                    input.pixels(),
                    width,
                    height,
                    &mut output_simd,
                    y,
                    &inverse,
                );

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

            warp_row_bilinear_simd(&input, width, height, &mut output_simd, y, &identity);
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
    fn test_warp_row_lanczos3_identity() {
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
    fn test_warp_row_lanczos3_various_sizes() {
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
    fn test_warp_image_lanczos3() {
        let width = 64;
        let height = 64;
        let input = patterns::diagonal_gradient(width, height);

        let transform = Transform::identity();

        let output = warp_image_lanczos3(input.pixels(), width, height, width, height, &transform);

        assert_eq!(output.len(), width * height);

        // Check interior pixels match input (within Lanczos tolerance)
        for y in 5..height - 5 {
            for x in 5..width - 5 {
                let expected = input[(x, y)];
                let actual = output[y * width + x];
                assert!(
                    (actual - expected).abs() < 0.02,
                    "({}, {}): {} vs {}",
                    x,
                    y,
                    actual,
                    expected
                );
            }
        }
    }
}
