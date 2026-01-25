//! Image interpolation for sub-pixel resampling.
//!
//! This module provides high-quality image interpolation using Lanczos resampling,
//! with bilinear and bicubic fallbacks for performance-critical applications.
//!
//! # Interpolation Methods
//!
//! - **Lanczos**: Highest quality, uses sinc function windowed by sinc. Best for
//!   astronomical images where preserving fine details matters.
//! - **Bicubic**: Good quality with reasonable performance. Uses cubic polynomials.
//! - **Bilinear**: Fast but lower quality. Linear interpolation in both dimensions.
//! - **Nearest**: Fastest, no interpolation. For previews or masks.

use std::f32::consts::PI;

use crate::registration::types::TransformMatrix;

/// Interpolation method for image resampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterpolationMethod {
    /// Nearest neighbor - fastest, lowest quality
    Nearest,
    /// Bilinear interpolation - fast, reasonable quality
    Bilinear,
    /// Bicubic interpolation - good quality
    Bicubic,
    /// Lanczos-2 (4x4 kernel) - high quality
    Lanczos2,
    /// Lanczos-3 (6x6 kernel) - highest quality, default
    #[default]
    Lanczos3,
    /// Lanczos-4 (8x8 kernel) - extreme quality
    Lanczos4,
}

impl InterpolationMethod {
    /// Returns the kernel radius for this interpolation method.
    #[inline]
    pub fn kernel_radius(&self) -> usize {
        match self {
            InterpolationMethod::Nearest => 1,
            InterpolationMethod::Bilinear => 1,
            InterpolationMethod::Bicubic => 2,
            InterpolationMethod::Lanczos2 => 2,
            InterpolationMethod::Lanczos3 => 3,
            InterpolationMethod::Lanczos4 => 4,
        }
    }
}

/// Configuration for image warping.
#[derive(Debug, Clone)]
pub struct WarpConfig {
    /// Interpolation method to use
    pub method: InterpolationMethod,
    /// Value to use for pixels outside the image bounds
    pub border_value: f32,
    /// Whether to normalize Lanczos kernel weights (recommended)
    pub normalize_kernel: bool,
}

impl Default for WarpConfig {
    fn default() -> Self {
        Self {
            method: InterpolationMethod::Lanczos3,
            border_value: 0.0,
            normalize_kernel: true,
        }
    }
}

/// Lanczos kernel value.
///
/// The Lanczos kernel is a sinc function windowed by another sinc:
/// L(x) = sinc(x) * sinc(x/a) for |x| < a
/// L(x) = 0 otherwise
///
/// where sinc(x) = sin(pi*x) / (pi*x) for x != 0, and sinc(0) = 1
#[inline]
fn lanczos_kernel(x: f32, a: f32) -> f32 {
    if x.abs() < 1e-6 {
        return 1.0;
    }
    if x.abs() >= a {
        return 0.0;
    }

    let pi_x = PI * x;
    let pi_x_a = pi_x / a;

    (pi_x.sin() / pi_x) * (pi_x_a.sin() / pi_x_a)
}

/// Bicubic kernel value (Catmull-Rom spline).
///
/// Uses the standard bicubic interpolation kernel:
/// W(x) = (a+2)|x|^3 - (a+3)|x|^2 + 1       for |x| <= 1
/// W(x) = a|x|^3 - 5a|x|^2 + 8a|x| - 4a     for 1 < |x| < 2
/// W(x) = 0                                  otherwise
///
/// where a = -0.5 for Catmull-Rom spline
#[inline]
fn bicubic_kernel(x: f32) -> f32 {
    const A: f32 = -0.5; // Catmull-Rom

    let abs_x = x.abs();

    if abs_x <= 1.0 {
        ((A + 2.0) * abs_x - (A + 3.0)) * abs_x * abs_x + 1.0
    } else if abs_x < 2.0 {
        ((A * abs_x - 5.0 * A) * abs_x + 8.0 * A) * abs_x - 4.0 * A
    } else {
        0.0
    }
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

/// Nearest neighbor interpolation.
#[inline]
fn interpolate_nearest(
    data: &[f32],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    border: f32,
) -> f32 {
    let ix = x.round() as i32;
    let iy = y.round() as i32;
    sample_pixel(data, width, height, ix, iy, border)
}

/// Bilinear interpolation.
#[inline]
fn interpolate_bilinear(
    data: &[f32],
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

    let p00 = sample_pixel(data, width, height, x0, y0, border);
    let p10 = sample_pixel(data, width, height, x1, y0, border);
    let p01 = sample_pixel(data, width, height, x0, y1, border);
    let p11 = sample_pixel(data, width, height, x1, y1, border);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);

    top + fy * (bottom - top)
}

/// Bicubic interpolation.
fn interpolate_bicubic(
    data: &[f32],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    border: f32,
) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    // Compute x weights
    let wx = [
        bicubic_kernel(fx + 1.0),
        bicubic_kernel(fx),
        bicubic_kernel(fx - 1.0),
        bicubic_kernel(fx - 2.0),
    ];

    // Compute y weights
    let wy = [
        bicubic_kernel(fy + 1.0),
        bicubic_kernel(fy),
        bicubic_kernel(fy - 1.0),
        bicubic_kernel(fy - 2.0),
    ];

    let mut sum = 0.0;

    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - 1 + j as i32;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - 1 + i as i32;
            let pixel = sample_pixel(data, width, height, px, py, border);
            sum += pixel * wxi * wyj;
        }
    }

    sum
}

/// Lanczos interpolation.
#[allow(clippy::too_many_arguments)]
fn interpolate_lanczos(
    data: &[f32],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    border: f32,
    a: usize,
    normalize: bool,
) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let a_i32 = a as i32;
    let a_f32 = a as f32;

    // Pre-compute x weights
    let mut wx = vec![0.0f32; 2 * a];
    for (i, w) in wx.iter_mut().enumerate() {
        let dx = fx - (i as i32 - a_i32 + 1) as f32;
        *w = lanczos_kernel(dx, a_f32);
    }

    // Pre-compute y weights
    let mut wy = vec![0.0f32; 2 * a];
    for (j, w) in wy.iter_mut().enumerate() {
        let dy = fy - (j as i32 - a_i32 + 1) as f32;
        *w = lanczos_kernel(dy, a_f32);
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
    let mut sum = 0.0;

    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - a_i32 + 1 + j as i32;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - a_i32 + 1 + i as i32;
            let pixel = sample_pixel(data, width, height, px, py, border);
            sum += pixel * wxi * wyj;
        }
    }

    sum
}

/// Interpolate a single pixel at sub-pixel coordinates.
pub fn interpolate_pixel(
    data: &[f32],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    config: &WarpConfig,
) -> f32 {
    match config.method {
        InterpolationMethod::Nearest => {
            interpolate_nearest(data, width, height, x, y, config.border_value)
        }
        InterpolationMethod::Bilinear => {
            interpolate_bilinear(data, width, height, x, y, config.border_value)
        }
        InterpolationMethod::Bicubic => {
            interpolate_bicubic(data, width, height, x, y, config.border_value)
        }
        InterpolationMethod::Lanczos2 => interpolate_lanczos(
            data,
            width,
            height,
            x,
            y,
            config.border_value,
            2,
            config.normalize_kernel,
        ),
        InterpolationMethod::Lanczos3 => interpolate_lanczos(
            data,
            width,
            height,
            x,
            y,
            config.border_value,
            3,
            config.normalize_kernel,
        ),
        InterpolationMethod::Lanczos4 => interpolate_lanczos(
            data,
            width,
            height,
            x,
            y,
            config.border_value,
            4,
            config.normalize_kernel,
        ),
    }
}

/// Warp an image using a transformation matrix.
///
/// Applies the inverse of the given transform to map output coordinates
/// to input coordinates, then interpolates the input image.
///
/// # Arguments
///
/// * `input` - Input image data (row-major, single channel)
/// * `input_width` - Width of input image
/// * `input_height` - Height of input image
/// * `output_width` - Width of output image
/// * `output_height` - Height of output image
/// * `transform` - Transformation from input to output coordinates
/// * `config` - Warp configuration
///
/// # Returns
///
/// Output image data (row-major)
pub fn warp_image(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    transform: &TransformMatrix,
    config: &WarpConfig,
) -> Vec<f32> {
    assert_eq!(
        input.len(),
        input_width * input_height,
        "Input size mismatch"
    );

    let mut output = vec![config.border_value; output_width * output_height];

    // Compute inverse transform to map output -> input
    let inverse = transform.inverse();

    for y in 0..output_height {
        for x in 0..output_width {
            // Transform output coordinates to input coordinates
            let (src_x, src_y) = inverse.transform_point(x as f64, y as f64);

            // Interpolate input at source coordinates
            output[y * output_width + x] = interpolate_pixel(
                input,
                input_width,
                input_height,
                src_x as f32,
                src_y as f32,
                config,
            );
        }
    }

    output
}

/// Resample an image to a new size using the specified interpolation.
pub fn resample_image(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    method: InterpolationMethod,
) -> Vec<f32> {
    let config = WarpConfig {
        method,
        border_value: 0.0,
        normalize_kernel: true,
    };

    let scale_x = input_width as f64 / output_width as f64;
    let scale_y = input_height as f64 / output_height as f64;

    // Create scaling transform (output to input)
    let transform = TransformMatrix::from_scale(1.0 / scale_x, 1.0 / scale_y);

    // We need the inverse since warp_image expects input->output transform
    let inverse_transform = transform.inverse();

    warp_image(
        input,
        input_width,
        input_height,
        output_width,
        output_height,
        &inverse_transform,
        &config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-5;

    #[test]
    fn test_lanczos_kernel_center() {
        // At center, kernel should be 1
        assert!((lanczos_kernel(0.0, 3.0) - 1.0).abs() < EPSILON);
    }

    #[test]
    fn test_lanczos_kernel_zeros() {
        // At integer positions, sinc is 0 (except at 0)
        assert!(lanczos_kernel(1.0, 3.0).abs() < EPSILON);
        assert!(lanczos_kernel(2.0, 3.0).abs() < EPSILON);
        assert!(lanczos_kernel(-1.0, 3.0).abs() < EPSILON);
    }

    #[test]
    fn test_lanczos_kernel_outside() {
        // Outside window, kernel is 0
        assert_eq!(lanczos_kernel(3.0, 3.0), 0.0);
        assert_eq!(lanczos_kernel(4.0, 3.0), 0.0);
        assert_eq!(lanczos_kernel(-3.0, 3.0), 0.0);
    }

    #[test]
    fn test_lanczos_kernel_symmetry() {
        // Kernel should be symmetric
        assert!((lanczos_kernel(0.5, 3.0) - lanczos_kernel(-0.5, 3.0)).abs() < EPSILON);
        assert!((lanczos_kernel(1.5, 3.0) - lanczos_kernel(-1.5, 3.0)).abs() < EPSILON);
    }

    #[test]
    fn test_bicubic_kernel_center() {
        assert!((bicubic_kernel(0.0) - 1.0).abs() < EPSILON);
    }

    #[test]
    fn test_bicubic_kernel_edges() {
        assert!(bicubic_kernel(2.0).abs() < EPSILON);
        assert!(bicubic_kernel(-2.0).abs() < EPSILON);
    }

    #[test]
    fn test_bicubic_kernel_continuity() {
        // Kernel should be continuous at x=1
        let left = bicubic_kernel(1.0 - 0.001);
        let right = bicubic_kernel(1.0 + 0.001);
        assert!((left - right).abs() < 0.01);
    }

    #[test]
    fn test_nearest_interpolation() {
        let data = vec![0.0, 1.0, 2.0, 3.0];
        let config = WarpConfig {
            method: InterpolationMethod::Nearest,
            ..Default::default()
        };

        // Center of pixels
        assert!((interpolate_pixel(&data, 2, 2, 0.4, 0.4, &config) - 0.0).abs() < EPSILON);
        assert!((interpolate_pixel(&data, 2, 2, 1.4, 0.4, &config) - 1.0).abs() < EPSILON);
        assert!((interpolate_pixel(&data, 2, 2, 0.4, 1.4, &config) - 2.0).abs() < EPSILON);
        assert!((interpolate_pixel(&data, 2, 2, 1.4, 1.4, &config) - 3.0).abs() < EPSILON);
    }

    #[test]
    fn test_bilinear_center() {
        let data = vec![0.0, 2.0, 2.0, 4.0];
        let config = WarpConfig {
            method: InterpolationMethod::Bilinear,
            ..Default::default()
        };

        // At pixel centers
        assert!((interpolate_pixel(&data, 2, 2, 0.0, 0.0, &config) - 0.0).abs() < EPSILON);
        assert!((interpolate_pixel(&data, 2, 2, 1.0, 0.0, &config) - 2.0).abs() < EPSILON);

        // Between pixels - should interpolate
        let center = interpolate_pixel(&data, 2, 2, 0.5, 0.5, &config);
        assert!((center - 2.0).abs() < EPSILON); // Average of all 4
    }

    #[test]
    fn test_bilinear_edge() {
        let data = vec![1.0, 1.0, 1.0, 1.0];
        let config = WarpConfig {
            method: InterpolationMethod::Bilinear,
            ..Default::default()
        };

        // Uniform image should give same value everywhere
        assert!((interpolate_pixel(&data, 2, 2, 0.3, 0.7, &config) - 1.0).abs() < EPSILON);
    }

    #[test]
    fn test_bicubic_pixel_centers() {
        // At pixel centers, bicubic should return exact values
        let data: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let config = WarpConfig {
            method: InterpolationMethod::Bicubic,
            ..Default::default()
        };

        assert!((interpolate_pixel(&data, 4, 4, 1.0, 1.0, &config) - 5.0).abs() < 0.01);
        assert!((interpolate_pixel(&data, 4, 4, 2.0, 2.0, &config) - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_lanczos_pixel_centers() {
        // At pixel centers, Lanczos should return exact values
        let data: Vec<f32> = (0..64).map(|i| i as f32).collect();
        let config = WarpConfig {
            method: InterpolationMethod::Lanczos3,
            normalize_kernel: true,
            ..Default::default()
        };

        assert!((interpolate_pixel(&data, 8, 8, 3.0, 3.0, &config) - 27.0).abs() < 0.1);
        assert!((interpolate_pixel(&data, 8, 8, 4.0, 4.0, &config) - 36.0).abs() < 0.1);
    }

    #[test]
    fn test_border_handling() {
        let data = vec![1.0; 4];
        let config = WarpConfig {
            method: InterpolationMethod::Bilinear,
            border_value: 0.0,
            ..Default::default()
        };

        // Inside - should interpolate to 1.0 since all pixels are 1.0
        assert!((interpolate_pixel(&data, 2, 2, 0.5, 0.5, &config) - 1.0).abs() < EPSILON);

        // Fully outside - should return border value
        assert!((interpolate_pixel(&data, 2, 2, -2.0, 0.0, &config) - 0.0).abs() < EPSILON);
    }

    #[test]
    fn test_warp_identity() {
        let input: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let transform = TransformMatrix::identity();
        let config = WarpConfig {
            method: InterpolationMethod::Bilinear,
            ..Default::default()
        };

        let output = warp_image(&input, 4, 4, 4, 4, &transform, &config);

        // Identity transform should preserve the image at pixel centers
        for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
            assert!(
                (inp - out).abs() < 0.1,
                "Mismatch at pixel {}: {} vs {}",
                i,
                inp,
                out
            );
        }
    }

    #[test]
    fn test_warp_translation() {
        // Create a simple image with a single bright pixel
        let mut input = vec![0.0f32; 16];
        input[5] = 1.0; // Position (1, 1)

        // Translate by (1, 1)
        let transform = TransformMatrix::from_translation(1.0, 1.0);
        let config = WarpConfig {
            method: InterpolationMethod::Bilinear,
            ..Default::default()
        };

        let output = warp_image(&input, 4, 4, 4, 4, &transform, &config);

        // The bright pixel should move to (2, 2)
        assert!(output[10] > 0.5, "Expected bright pixel at (2,2)");
        assert!(output[5] < 0.1, "Expected dark pixel at (1,1)");
    }

    #[test]
    fn test_warp_scale() {
        // Create a 2x2 image
        let input = vec![1.0, 2.0, 3.0, 4.0];

        // Scale 2x
        let transform = TransformMatrix::from_scale(2.0, 2.0);
        let config = WarpConfig {
            method: InterpolationMethod::Bilinear,
            ..Default::default()
        };

        let output = warp_image(&input, 2, 2, 4, 4, &transform, &config);

        assert_eq!(output.len(), 16);
        // Top-left corner should still be ~1.0
        assert!((output[0] - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_resample_upscale() {
        let input = vec![1.0, 2.0, 3.0, 4.0];
        let output = resample_image(&input, 2, 2, 4, 4, InterpolationMethod::Bilinear);

        assert_eq!(output.len(), 16);
        // Corners should preserve values
        assert!((output[0] - 1.0).abs() < 0.2);
    }

    #[test]
    fn test_resample_downscale() {
        // Create larger image
        let input: Vec<f32> = (0..64).map(|i| i as f32).collect();
        let output = resample_image(&input, 8, 8, 4, 4, InterpolationMethod::Lanczos3);

        assert_eq!(output.len(), 16);
    }

    #[test]
    fn test_interpolation_method_radius() {
        assert_eq!(InterpolationMethod::Nearest.kernel_radius(), 1);
        assert_eq!(InterpolationMethod::Bilinear.kernel_radius(), 1);
        assert_eq!(InterpolationMethod::Bicubic.kernel_radius(), 2);
        assert_eq!(InterpolationMethod::Lanczos2.kernel_radius(), 2);
        assert_eq!(InterpolationMethod::Lanczos3.kernel_radius(), 3);
        assert_eq!(InterpolationMethod::Lanczos4.kernel_radius(), 4);
    }

    #[test]
    fn test_lanczos_preserves_dc() {
        // A uniform image should remain uniform after Lanczos interpolation
        let input = vec![0.5f32; 64];
        let config = WarpConfig {
            method: InterpolationMethod::Lanczos3,
            normalize_kernel: true,
            ..Default::default()
        };

        // Sample at various sub-pixel positions
        let val1 = interpolate_pixel(&input, 8, 8, 3.3, 4.7, &config);
        let val2 = interpolate_pixel(&input, 8, 8, 2.1, 5.9, &config);

        assert!((val1 - 0.5).abs() < 0.01);
        assert!((val2 - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_warp_rotation() {
        // Create image with asymmetric pattern
        let mut input = vec![0.0f32; 64];
        input[0] = 1.0; // Top-left corner

        // Rotate 90 degrees around center
        let transform =
            TransformMatrix::from_rotation_around(std::f64::consts::FRAC_PI_2, 4.0, 4.0);
        let config = WarpConfig {
            method: InterpolationMethod::Bilinear,
            ..Default::default()
        };

        let output = warp_image(&input, 8, 8, 8, 8, &transform, &config);

        // After 90 degree rotation, top-left should move
        // The bright pixel should be somewhere else
        assert!(
            output[0] < 0.5,
            "Top-left should not be bright after rotation"
        );
    }

    #[test]
    fn test_bicubic_smooth_gradient() {
        // Bicubic should smoothly interpolate gradients
        let input: Vec<f32> = (0..16).map(|i| (i % 4) as f32).collect();
        let config = WarpConfig {
            method: InterpolationMethod::Bicubic,
            ..Default::default()
        };

        // Sample between pixels
        let v1 = interpolate_pixel(&input, 4, 4, 0.5, 1.0, &config);
        let v2 = interpolate_pixel(&input, 4, 4, 1.5, 1.0, &config);
        let v3 = interpolate_pixel(&input, 4, 4, 2.5, 1.0, &config);

        // Should be monotonically increasing in a gradient
        assert!(v1 < v2);
        assert!(v2 < v3);
    }

    #[test]
    fn test_lanczos2_vs_lanczos3() {
        let input: Vec<f32> = (0..64).map(|i| (i as f32).sin()).collect();

        let config2 = WarpConfig {
            method: InterpolationMethod::Lanczos2,
            ..Default::default()
        };
        let config3 = WarpConfig {
            method: InterpolationMethod::Lanczos3,
            ..Default::default()
        };

        let v2 = interpolate_pixel(&input, 8, 8, 3.5, 4.5, &config2);
        let v3 = interpolate_pixel(&input, 8, 8, 3.5, 4.5, &config3);

        // Both should give reasonable values (not wildly different)
        assert!((v2 - v3).abs() < 0.5);
    }
}
