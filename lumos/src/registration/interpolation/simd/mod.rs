//! SIMD-accelerated interpolation utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE4.1 on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms
//!
//! # Supported Interpolation Methods
//!
//! - **Bilinear**: SIMD-accelerated on all platforms (8 pixels/cycle on AVX2)
//! - **Lanczos3**: SIMD-accelerated on x86_64 (AVX2/SSE) and aarch64 (NEON)

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

use crate::registration::interpolation::lanczos_kernel;
use crate::registration::types::TransformMatrix;

/// Warp a row of pixels using SIMD-accelerated bilinear interpolation.
///
/// This processes multiple output pixels in parallel by:
/// 1. Computing source coordinates for multiple pixels at once
/// 2. Gathering pixel values using SIMD
/// 3. Computing bilinear weights and interpolating in parallel
///
/// # Arguments
/// * `input` - Input image data (row-major)
/// * `input_width` - Width of input image
/// * `input_height` - Height of input image
/// * `output_row` - Output buffer for this row
/// * `output_y` - Y coordinate of this row
/// * `inverse` - Inverse transform (output -> input)
/// * `border_value` - Value for out-of-bounds pixels
#[inline]
pub fn warp_row_bilinear_simd(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &TransformMatrix,
    border_value: f32,
) {
    let output_width = output_row.len();

    #[cfg(target_arch = "x86_64")]
    {
        if output_width >= 8 && cpu_features::has_avx2() {
            unsafe {
                sse::warp_row_bilinear_avx2(
                    input,
                    input_width,
                    input_height,
                    output_row,
                    output_y,
                    inverse,
                    border_value,
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
                    border_value,
                );
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if output_width >= 4 {
            unsafe {
                neon::warp_row_bilinear_neon(
                    input,
                    input_width,
                    input_height,
                    output_row,
                    output_y,
                    inverse,
                    border_value,
                );
            }
            return;
        }
    }

    // Scalar fallback
    warp_row_bilinear_scalar(
        input,
        input_width,
        input_height,
        output_row,
        output_y,
        inverse,
        border_value,
    );
}

/// Scalar implementation of row warping with bilinear interpolation.
pub fn warp_row_bilinear_scalar(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &TransformMatrix,
    border_value: f32,
) {
    let y = output_y as f64;

    for (x, out_pixel) in output_row.iter_mut().enumerate() {
        let (src_x, src_y) = inverse.transform_point(x as f64, y);

        *out_pixel = bilinear_sample(
            input,
            input_width,
            input_height,
            src_x as f32,
            src_y as f32,
            border_value,
        );
    }
}

/// Bilinear sampling at a single point (f32 coordinates).
///
/// This is the SIMD-compatible implementation using f32 coordinates for fast warping.
/// A similar implementation exists in `phase_correlation/mod.rs` using f64 coordinates
/// for higher precision in log-polar transforms.
#[inline]
pub fn bilinear_sample(
    input: &[f32],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    border: f32,
) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = sample_pixel(input, width, height, x0, y0, border);
    let p10 = sample_pixel(input, width, height, x1, y0, border);
    let p01 = sample_pixel(input, width, height, x0, y1, border);
    let p11 = sample_pixel(input, width, height, x1, y1, border);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);

    top + fy * (bottom - top)
}

/// Sample a pixel with bounds checking.
#[inline]
fn sample_pixel(data: &[f32], width: usize, height: usize, x: i32, y: i32, border: f32) -> f32 {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        border
    } else {
        data[y as usize * width + x as usize]
    }
}

/// Warp a row of pixels using SIMD-accelerated Lanczos3 interpolation.
///
/// Lanczos3 uses a 6x6 kernel for high-quality resampling. This implementation
/// accelerates the weight computation and accumulation using SIMD.
///
/// # Arguments
/// * `input` - Input image data (row-major)
/// * `input_width` - Width of input image
/// * `input_height` - Height of input image
/// * `output_row` - Output buffer for this row
/// * `output_y` - Y coordinate of this row
/// * `inverse` - Inverse transform (output -> input)
/// * `border_value` - Value for out-of-bounds pixels
/// * `normalize` - Whether to normalize kernel weights
/// * `clamp` - Whether to clamp output to neighborhood min/max
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn warp_row_lanczos3_simd(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &TransformMatrix,
    border_value: f32,
    normalize: bool,
    clamp: bool,
) {
    let output_width = output_row.len();

    #[cfg(target_arch = "x86_64")]
    {
        if output_width >= 4 && cpu_features::has_avx2() {
            unsafe {
                sse::warp_row_lanczos3_avx2(
                    input,
                    input_width,
                    input_height,
                    output_row,
                    output_y,
                    inverse,
                    border_value,
                    normalize,
                    clamp,
                );
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if output_width >= 4 {
            unsafe {
                neon::warp_row_lanczos3_neon(
                    input,
                    input_width,
                    input_height,
                    output_row,
                    output_y,
                    inverse,
                    border_value,
                    normalize,
                    clamp,
                );
            }
            return;
        }
    }

    // Scalar fallback
    warp_row_lanczos3_scalar(
        input,
        input_width,
        input_height,
        output_row,
        output_y,
        inverse,
        border_value,
        normalize,
        clamp,
    );
}

/// Scalar implementation of row warping with Lanczos3 interpolation.
#[allow(clippy::too_many_arguments)]
pub fn warp_row_lanczos3_scalar(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &TransformMatrix,
    border_value: f32,
    normalize: bool,
    clamp: bool,
) {
    let y = output_y as f64;
    const A: usize = 3; // Lanczos3 kernel radius

    for (x, out_pixel) in output_row.iter_mut().enumerate() {
        let (src_x, src_y) = inverse.transform_point(x as f64, y);
        let sx = src_x as f32;
        let sy = src_y as f32;

        let x0 = sx.floor() as i32;
        let y0 = sy.floor() as i32;
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;

        // Pre-compute x weights (6 values for Lanczos3)
        let mut wx = [0.0f32; 6];
        for (i, w) in wx.iter_mut().enumerate() {
            let dx = fx - (i as i32 - 2) as f32;
            *w = lanczos_kernel(dx, A as f32);
        }

        // Pre-compute y weights
        let mut wy = [0.0f32; 6];
        for (j, w) in wy.iter_mut().enumerate() {
            let dy = fy - (j as i32 - 2) as f32;
            *w = lanczos_kernel(dy, A as f32);
        }

        // Normalize weights if requested
        if normalize {
            let wx_sum: f32 = wx.iter().sum();
            let wy_sum: f32 = wy.iter().sum();
            if wx_sum.abs() > 1e-10 {
                wx.iter_mut().for_each(|w| *w /= wx_sum);
            }
            if wy_sum.abs() > 1e-10 {
                wy.iter_mut().for_each(|w| *w /= wy_sum);
            }
        }

        // Compute weighted sum
        let mut sum = 0.0f32;
        let mut min_val = f32::MAX;
        let mut max_val = f32::MIN;

        for (j, &wyj) in wy.iter().enumerate() {
            let py = y0 - 2 + j as i32;
            for (i, &wxi) in wx.iter().enumerate() {
                let px = x0 - 2 + i as i32;
                let pixel = sample_pixel(input, input_width, input_height, px, py, border_value);
                sum += pixel * wxi * wyj;

                if clamp {
                    min_val = min_val.min(pixel);
                    max_val = max_val.max(pixel);
                }
            }
        }

        *out_pixel = if clamp && min_val <= max_val {
            sum.clamp(min_val, max_val)
        } else {
            sum
        };
    }
}

/// Warp an entire image using SIMD-accelerated Lanczos3 interpolation.
#[allow(clippy::too_many_arguments)]
pub fn warp_image_lanczos3_simd(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    transform: &TransformMatrix,
    border_value: f32,
    normalize: bool,
    clamp: bool,
) -> Vec<f32> {
    let mut output = vec![border_value; output_width * output_height];
    let inverse = transform.inverse();

    for y in 0..output_height {
        let row_start = y * output_width;
        let row_end = row_start + output_width;
        warp_row_lanczos3_simd(
            input,
            input_width,
            input_height,
            &mut output[row_start..row_end],
            y,
            &inverse,
            border_value,
            normalize,
            clamp,
        );
    }

    output
}

/// Warp an entire image using SIMD-accelerated bilinear interpolation.
///
/// This is a convenience function that processes all rows.
pub fn warp_image_bilinear_simd(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    transform: &TransformMatrix,
    border_value: f32,
) -> Vec<f32> {
    let mut output = vec![border_value; output_width * output_height];
    let inverse = transform.inverse();

    for y in 0..output_height {
        let row_start = y * output_width;
        let row_end = row_start + output_width;
        warp_row_bilinear_simd(
            input,
            input_width,
            input_height,
            &mut output[row_start..row_end],
            y,
            &inverse,
            border_value,
        );
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_image(width: usize, height: usize) -> Vec<f32> {
        // Create gradient image
        (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| (x as f32 + y as f32 * 0.5) / (width + height) as f32)
            })
            .collect()
    }

    #[test]
    fn test_warp_row_bilinear_identity() {
        let width = 100;
        let height = 100;
        let input = create_test_image(width, height);
        let identity = TransformMatrix::identity();

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_bilinear_simd(&input, width, height, &mut output_row, y, &identity, 0.0);

        // With identity transform, output should match input
        for x in 1..width - 1 {
            let expected = input[y * width + x];
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
        let input = create_test_image(width, height);

        // Translate by (5, 3)
        let transform = TransformMatrix::translation(5.0, 3.0);
        let inverse = transform.inverse();

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_bilinear_simd(&input, width, height, &mut output_row, y, &inverse, -1.0);

        // Check that pixels are shifted
        for (x, &output_val) in output_row.iter().enumerate().skip(10).take(width - 20) {
            // Output at (x, y) should come from input at (x-5, y-3)
            let src_x = x as i32 - 5;
            let src_y = y as i32 - 3;
            if src_x >= 0 && src_y >= 0 {
                let expected = input[src_y as usize * width + src_x as usize];
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
        let input = create_test_image(width, height);

        // Test with various transforms
        let transforms = vec![
            TransformMatrix::identity(),
            TransformMatrix::translation(2.5, 1.7),
            TransformMatrix::similarity(3.0, 2.0, 0.1, 1.05),
        ];

        for transform in transforms {
            let inverse = transform.inverse();

            for y in [0, 50, height - 1] {
                let mut output_simd = vec![0.0f32; width];
                let mut output_scalar = vec![0.0f32; width];

                warp_row_bilinear_simd(&input, width, height, &mut output_simd, y, &inverse, -1.0);

                warp_row_bilinear_scalar(
                    &input,
                    width,
                    height,
                    &mut output_scalar,
                    y,
                    &inverse,
                    -1.0,
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
    fn test_warp_image_bilinear_simd() {
        let width = 64;
        let height = 64;
        let input = create_test_image(width, height);

        let transform = TransformMatrix::translation(1.0, 1.0);

        let output =
            warp_image_bilinear_simd(&input, width, height, width, height, &transform, 0.0);

        assert_eq!(output.len(), width * height);

        // Check a few interior pixels
        for y in 5..height - 5 {
            for x in 5..width - 5 {
                let expected = input[(y - 1) * width + (x - 1)];
                let actual = output[y * width + x];
                assert!(
                    (actual - expected).abs() < 0.01,
                    "({}, {}): {} vs {}",
                    x,
                    y,
                    actual,
                    expected
                );
            }
        }
    }

    #[test]
    fn test_warp_row_various_sizes() {
        let height = 64;
        let input_base = create_test_image(256, height);

        // Test various widths including non-SIMD-aligned sizes
        for width in [
            1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 100, 128,
        ] {
            let input: Vec<f32> = input_base.iter().take(width * height).copied().collect();
            let identity = TransformMatrix::identity();

            let mut output_simd = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            let y = height / 2;

            warp_row_bilinear_simd(&input, width, height, &mut output_simd, y, &identity, 0.0);
            warp_row_bilinear_scalar(&input, width, height, &mut output_scalar, y, &identity, 0.0);

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
        let input = create_test_image(width, height);
        let identity = TransformMatrix::identity();

        let mut output_row = vec![0.0f32; width];
        let y = 50;

        warp_row_lanczos3_simd(
            &input,
            width,
            height,
            &mut output_row,
            y,
            &identity,
            0.0,
            true,
            false,
        );

        // With identity transform, output should match input (within Lanczos ringing tolerance)
        for x in 3..width - 3 {
            let expected = input[y * width + x];
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
    fn test_warp_row_lanczos3_simd_matches_scalar() {
        let width = 128;
        let height = 128;
        let input = create_test_image(width, height);

        // Test with various transforms
        let transforms = vec![
            TransformMatrix::identity(),
            TransformMatrix::translation(2.5, 1.7),
            TransformMatrix::similarity(3.0, 2.0, 0.1, 1.05),
        ];

        for transform in transforms {
            let inverse = transform.inverse();

            for y in [3, 50, height - 4] {
                for (normalize, clamp) in [(false, false), (true, false), (true, true)] {
                    let mut output_simd = vec![0.0f32; width];
                    let mut output_scalar = vec![0.0f32; width];

                    warp_row_lanczos3_simd(
                        &input,
                        width,
                        height,
                        &mut output_simd,
                        y,
                        &inverse,
                        -1.0,
                        normalize,
                        clamp,
                    );

                    warp_row_lanczos3_scalar(
                        &input,
                        width,
                        height,
                        &mut output_scalar,
                        y,
                        &inverse,
                        -1.0,
                        normalize,
                        clamp,
                    );

                    for x in 0..width {
                        assert!(
                            (output_simd[x] - output_scalar[x]).abs() < 1e-4,
                            "Row {}, x={}, normalize={}, clamp={}: SIMD {} vs Scalar {}",
                            y,
                            x,
                            normalize,
                            clamp,
                            output_simd[x],
                            output_scalar[x]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_warp_row_lanczos3_various_sizes() {
        let height = 64;
        let input_base = create_test_image(256, height);

        // Test various widths including non-SIMD-aligned sizes
        for width in [1, 2, 3, 4, 5, 7, 8, 16, 33, 64, 100] {
            let input: Vec<f32> = input_base.iter().take(width * height).copied().collect();
            let transform = TransformMatrix::translation(1.5, 0.5);
            let inverse = transform.inverse();

            let mut output_simd = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            let y = height / 2;

            warp_row_lanczos3_simd(
                &input,
                width,
                height,
                &mut output_simd,
                y,
                &inverse,
                0.0,
                true,
                false,
            );
            warp_row_lanczos3_scalar(
                &input,
                width,
                height,
                &mut output_scalar,
                y,
                &inverse,
                0.0,
                true,
                false,
            );

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
    fn test_warp_image_lanczos3_simd() {
        let width = 64;
        let height = 64;
        let input = create_test_image(width, height);

        let transform = TransformMatrix::identity();

        let output = warp_image_lanczos3_simd(
            &input, width, height, width, height, &transform, 0.0, true, false,
        );

        assert_eq!(output.len(), width * height);

        // Check interior pixels match input (within Lanczos tolerance)
        for y in 5..height - 5 {
            for x in 5..width - 5 {
                let expected = input[y * width + x];
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
