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

#[cfg(test)]
mod tests;

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
pub(crate) fn lanczos_kernel(x: f32, a: f32) -> f32 {
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
pub(crate) fn bicubic_kernel(x: f32) -> f32 {
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
