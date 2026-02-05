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
//!   SIMD-accelerated on x86_64 (AVX2/SSE4.1) and aarch64 (NEON).
//! - **Nearest**: Fastest, no interpolation. For previews or masks.

use std::f32::consts::PI;
use std::sync::OnceLock;

use rayon::prelude::*;

use crate::common::Buffer2;
use crate::registration::config::InterpolationMethod;
use crate::registration::transform::Transform;

/// Number of rows to process per parallel chunk.
/// Balances parallelism overhead vs cache locality.
const ROWS_PER_CHUNK: usize = 32;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

pub mod simd;

/// Lanczos kernel value (direct computation).
///
/// The Lanczos kernel is a sinc function windowed by another sinc:
/// L(x) = sinc(x) * sinc(x/a) for |x| < a
/// L(x) = 0 otherwise
///
/// where sinc(x) = sin(pi*x) / (pi*x) for x != 0, and sinc(0) = 1
#[inline]
fn lanczos_kernel_direct(x: f32, a: f32) -> f32 {
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

// ============================================================================
// Lanczos Kernel Lookup Table (LUT) for Performance Optimization
// ============================================================================

/// Number of sub-pixel samples per unit interval in the LUT.
/// 4096 gives ~0.00024 precision - sufficient for direct lookup without interpolation.
/// LUT size for Lanczos3: 4096 * 3 * 4 bytes = 48KB (fits in L1 cache).
const LANCZOS_LUT_RESOLUTION: usize = 4096;

/// Pre-computed Lanczos kernel lookup table.
///
/// The LUT covers the range [0, a] with LANCZOS_LUT_RESOLUTION samples per unit.
/// Total entries = a * LANCZOS_LUT_RESOLUTION + 1 (for endpoint).
///
/// Since the kernel is symmetric, we only store positive values and use abs(x).
#[derive(Debug)]
struct LanczosLut {
    /// Pre-computed kernel values for x in [0, a]
    values: Vec<f32>,
    /// Kernel parameter (2, 3, or 4)
    a: usize,
}

impl LanczosLut {
    /// Create a new LUT for the given kernel parameter.
    fn new(a: usize) -> Self {
        let num_entries = a * LANCZOS_LUT_RESOLUTION + 1;
        let mut values = Vec::with_capacity(num_entries);

        let a_f32 = a as f32;
        for i in 0..num_entries {
            let x = i as f32 / LANCZOS_LUT_RESOLUTION as f32;
            values.push(lanczos_kernel_direct(x, a_f32));
        }

        Self { values, a }
    }

    /// Look up the kernel value for a given x using direct table access.
    /// With 4096 samples per unit, rounding error is negligible.
    #[inline]
    fn lookup(&self, x: f32) -> f32 {
        let abs_x = x.abs();
        let a_f32 = self.a as f32;

        if abs_x >= a_f32 {
            return 0.0;
        }

        // Direct index with rounding (faster than interpolation)
        let idx = (abs_x * LANCZOS_LUT_RESOLUTION as f32 + 0.5) as usize;
        // Safety: idx is bounded by a_f32 * RESOLUTION which is < values.len()
        unsafe { *self.values.get_unchecked(idx) }
    }
}

// Global LUT instances (lazily initialized)
static LANCZOS2_LUT: OnceLock<LanczosLut> = OnceLock::new();
static LANCZOS3_LUT: OnceLock<LanczosLut> = OnceLock::new();
static LANCZOS4_LUT: OnceLock<LanczosLut> = OnceLock::new();

/// Get the LUT for a given kernel parameter.
#[inline]
fn get_lanczos_lut(a: usize) -> &'static LanczosLut {
    match a {
        2 => LANCZOS2_LUT.get_or_init(|| LanczosLut::new(2)),
        3 => LANCZOS3_LUT.get_or_init(|| LanczosLut::new(3)),
        4 => LANCZOS4_LUT.get_or_init(|| LanczosLut::new(4)),
        _ => panic!("Unsupported Lanczos parameter: {}", a),
    }
}

/// Lanczos kernel value using lookup table.
///
/// Uses pre-computed LUT with linear interpolation for fast evaluation.
/// Falls back to direct computation for unsupported kernel sizes.
#[inline]
pub(crate) fn lanczos_kernel(x: f32, a: f32) -> f32 {
    let a_usize = a as usize;
    if (2..=4).contains(&a_usize) && (a - a_usize as f32).abs() < 1e-6 {
        get_lanczos_lut(a_usize).lookup(x)
    } else {
        lanczos_kernel_direct(x, a)
    }
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
fn sample_pixel(data: &Buffer2<f32>, x: i32, y: i32, border: f32) -> f32 {
    if x < 0 || y < 0 || x >= data.width() as i32 || y >= data.height() as i32 {
        border
    } else {
        data[y as usize * data.width() + x as usize]
    }
}

/// Nearest neighbor interpolation.
#[inline]
fn interpolate_nearest(data: &Buffer2<f32>, x: f32, y: f32, border: f32) -> f32 {
    let ix = x.round() as i32;
    let iy = y.round() as i32;
    sample_pixel(data, ix, iy, border)
}

/// Bilinear interpolation.
#[inline]
fn interpolate_bilinear(data: &Buffer2<f32>, x: f32, y: f32, border: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = sample_pixel(data, x0, y0, border);
    let p10 = sample_pixel(data, x1, y0, border);
    let p01 = sample_pixel(data, x0, y1, border);
    let p11 = sample_pixel(data, x1, y1, border);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);

    top + fy * (bottom - top)
}

/// Bicubic interpolation.
fn interpolate_bicubic(data: &Buffer2<f32>, x: f32, y: f32, border: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let wx = [
        bicubic_kernel(fx + 1.0),
        bicubic_kernel(fx),
        bicubic_kernel(fx - 1.0),
        bicubic_kernel(fx - 2.0),
    ];

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
            sum += sample_pixel(data, px, py, border) * wxi * wyj;
        }
    }

    sum
}

/// Lanczos interpolation with stack-allocated weights (no heap allocation).
#[inline]
fn interpolate_lanczos(data: &Buffer2<f32>, x: f32, y: f32, a: usize) -> f32 {
    match a {
        2 => interpolate_lanczos_impl::<2, 4>(data, x, y),
        3 => interpolate_lanczos_impl::<3, 6>(data, x, y),
        4 => interpolate_lanczos_impl::<4, 8>(data, x, y),
        _ => interpolate_lanczos_generic(data, x, y, a),
    }
}

/// Generic Lanczos for unsupported kernel sizes (rare).
fn interpolate_lanczos_generic(data: &Buffer2<f32>, x: f32, y: f32, a: usize) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let a_i32 = a as i32;
    let a_f32 = a as f32;

    let mut wx = vec![0.0f32; 2 * a];
    let mut wy = vec![0.0f32; 2 * a];

    for (i, w) in wx.iter_mut().enumerate() {
        *w = lanczos_kernel(fx - (i as i32 - a_i32 + 1) as f32, a_f32);
    }
    for (j, w) in wy.iter_mut().enumerate() {
        *w = lanczos_kernel(fy - (j as i32 - a_i32 + 1) as f32, a_f32);
    }

    let wx_sum: f32 = wx.iter().sum();
    let wy_sum: f32 = wy.iter().sum();
    if wx_sum.abs() > 1e-10 {
        wx.iter_mut().for_each(|w| *w /= wx_sum);
    }
    if wy_sum.abs() > 1e-10 {
        wy.iter_mut().for_each(|w| *w /= wy_sum);
    }

    let mut sum = 0.0;
    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - a_i32 + 1 + j as i32;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - a_i32 + 1 + i as i32;
            sum += sample_pixel(data, px, py, 0.0) * wxi * wyj;
        }
    }
    sum
}

/// Optimized Lanczos with compile-time kernel size (stack-allocated weights).
#[inline]
fn interpolate_lanczos_impl<const A: usize, const SIZE: usize>(
    data: &Buffer2<f32>,
    x: f32,
    y: f32,
) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let a_i32 = A as i32;
    let lut = get_lanczos_lut(A);

    let mut wx = [0.0f32; SIZE];
    let mut wy = [0.0f32; SIZE];

    let mut wx_sum = 0.0f32;
    let mut wy_sum = 0.0f32;

    for (i, w) in wx.iter_mut().enumerate() {
        let dx = fx - (i as i32 - a_i32 + 1) as f32;
        *w = lut.lookup(dx);
        wx_sum += *w;
    }

    for (j, w) in wy.iter_mut().enumerate() {
        let dy = fy - (j as i32 - a_i32 + 1) as f32;
        *w = lut.lookup(dy);
        wy_sum += *w;
    }

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

    let mut sum = 0.0f32;

    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - a_i32 + 1 + j as i32;
        let wyj = wyj * inv_wy;

        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - a_i32 + 1 + i as i32;
            let wxi = wxi * inv_wx;
            sum += sample_pixel(data, px, py, 0.0) * wxi * wyj;
        }
    }

    sum
}

/// Interpolate a single pixel at sub-pixel coordinates.
#[cfg(test)]
pub fn interpolate_pixel(data: &Buffer2<f32>, x: f32, y: f32, method: InterpolationMethod) -> f32 {
    match method {
        InterpolationMethod::Nearest => interpolate_nearest(data, x, y, 0.0),
        InterpolationMethod::Bilinear => interpolate_bilinear(data, x, y, 0.0),
        InterpolationMethod::Bicubic => interpolate_bicubic(data, x, y, 0.0),
        InterpolationMethod::Lanczos2 => interpolate_lanczos(data, x, y, 2),
        InterpolationMethod::Lanczos3 => interpolate_lanczos(data, x, y, 3),
        InterpolationMethod::Lanczos4 => interpolate_lanczos(data, x, y, 4),
    }
}

/// Warp an image using a transformation matrix.
///
/// Applies the inverse of the given transform to map output coordinates
/// to input coordinates, then interpolates the input image.
///
/// This function is parallelized using rayon, processing rows in chunks.
/// For bilinear interpolation, it also uses SIMD acceleration when available:
/// - AVX2/SSE4.1 on x86_64
/// - NEON on aarch64
///
/// Input and output must have the same dimensions.
pub fn warp_image(
    input: &Buffer2<f32>,
    output: &mut Buffer2<f32>,
    transform: &Transform,
    method: InterpolationMethod,
) {
    let width = input.width();
    let height = input.height();
    debug_assert_eq!(width, output.width());
    debug_assert_eq!(height, output.height());

    let inverse = transform.inverse();

    // Use SIMD-accelerated path for bilinear interpolation
    if method == InterpolationMethod::Bilinear {
        output
            .pixels_mut()
            .par_chunks_mut(width * ROWS_PER_CHUNK)
            .enumerate()
            .for_each(|(chunk_idx, chunk): (usize, &mut [f32])| {
                let start_y = chunk_idx * ROWS_PER_CHUNK;
                let num_rows = chunk.len() / width;

                for row_in_chunk in 0..num_rows {
                    let y = start_y + row_in_chunk;
                    let row_start = row_in_chunk * width;
                    let row_end = row_start + width;
                    simd::warp_row_bilinear_simd(
                        input,
                        width,
                        height,
                        &mut chunk[row_start..row_end],
                        y,
                        &inverse,
                    );
                }
            });
        return;
    }

    // Parallel path for other interpolation methods (Lanczos, Bicubic, etc.)
    output
        .pixels_mut()
        .par_chunks_mut(width * ROWS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk_idx, chunk): (usize, &mut [f32])| {
            let start_y = chunk_idx * ROWS_PER_CHUNK;
            let num_rows = chunk.len() / width;

            for row_in_chunk in 0..num_rows {
                let y = start_y + row_in_chunk;
                for x in 0..width {
                    let src = inverse.apply(glam::DVec2::new(x as f64, y as f64));
                    chunk[row_in_chunk * width + x] =
                        interpolate_pixel_internal(input, src.x as f32, src.y as f32, method);
                }
            }
        });
}

#[inline]
fn interpolate_pixel_internal(
    data: &Buffer2<f32>,
    x: f32,
    y: f32,
    method: InterpolationMethod,
) -> f32 {
    match method {
        InterpolationMethod::Nearest => interpolate_nearest(data, x, y, 0.0),
        InterpolationMethod::Bilinear => interpolate_bilinear(data, x, y, 0.0),
        InterpolationMethod::Bicubic => interpolate_bicubic(data, x, y, 0.0),
        InterpolationMethod::Lanczos2 => interpolate_lanczos(data, x, y, 2),
        InterpolationMethod::Lanczos3 => interpolate_lanczos(data, x, y, 3),
        InterpolationMethod::Lanczos4 => interpolate_lanczos(data, x, y, 4),
    }
}
