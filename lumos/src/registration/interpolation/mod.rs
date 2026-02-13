//! Image interpolation for sub-pixel resampling.
//!
//! Provides Lanczos, bicubic, bilinear, and nearest-neighbor interpolation.
//! Bilinear and Lanczos3 have optimized row-warping paths (AVX2/SSE4.1 on x86_64,
//! scalar with incremental stepping on aarch64).

use std::f32::consts::PI;
use std::sync::OnceLock;

use rayon::prelude::*;

use crate::common::Buffer2;
use crate::registration::config::InterpolationMethod;
use crate::registration::transform::WarpTransform;

/// Bundled warp parameters passed through the interpolation pipeline.
#[derive(Debug, Clone, Copy)]
pub struct WarpParams {
    pub method: InterpolationMethod,
    pub border_value: f32,
}

impl Default for WarpParams {
    fn default() -> Self {
        Self {
            method: InterpolationMethod::default(),
            border_value: 0.0,
        }
    }
}

impl WarpParams {
    #[allow(dead_code)]
    pub fn new(method: InterpolationMethod) -> Self {
        Self {
            method,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

pub mod warp;

// Lanczos LUT: 4096 samples/unit gives ~0.00024 precision.
// Lanczos3 LUT: 4096 * 3 * 4 bytes = 48KB (fits in L1 cache).
const LANCZOS_LUT_RESOLUTION: usize = 4096;

/// Direct Lanczos kernel computation (used for LUT initialization).
#[inline]
fn lanczos_kernel_compute(x: f32, a: f32) -> f32 {
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

#[derive(Debug)]
pub(super) struct LanczosLut {
    values: Vec<f32>,
    a: usize,
}

impl LanczosLut {
    fn new(a: usize) -> Self {
        let num_entries = a * LANCZOS_LUT_RESOLUTION + 1;
        let a_f32 = a as f32;
        let values = (0..num_entries)
            .map(|i| {
                let x = i as f32 / LANCZOS_LUT_RESOLUTION as f32;
                lanczos_kernel_compute(x, a_f32)
            })
            .collect();
        Self { values, a }
    }

    #[inline]
    pub(super) fn lookup(&self, x: f32) -> f32 {
        let abs_x = x.abs();
        if abs_x >= self.a as f32 {
            return 0.0;
        }
        let idx = (abs_x * LANCZOS_LUT_RESOLUTION as f32 + 0.5) as usize;
        unsafe { *self.values.get_unchecked(idx) }
    }
}

static LANCZOS2_LUT: OnceLock<LanczosLut> = OnceLock::new();
static LANCZOS3_LUT: OnceLock<LanczosLut> = OnceLock::new();
static LANCZOS4_LUT: OnceLock<LanczosLut> = OnceLock::new();

#[inline]
pub(super) fn get_lanczos_lut(a: usize) -> &'static LanczosLut {
    match a {
        2 => LANCZOS2_LUT.get_or_init(|| LanczosLut::new(2)),
        3 => LANCZOS3_LUT.get_or_init(|| LanczosLut::new(3)),
        4 => LANCZOS4_LUT.get_or_init(|| LanczosLut::new(4)),
        _ => panic!("Unsupported Lanczos parameter: {a}"),
    }
}

/// Bicubic kernel (Catmull-Rom, a = -0.5).
#[inline]
pub(crate) fn bicubic_kernel(x: f32) -> f32 {
    const A: f32 = -0.5;
    let abs_x = x.abs();
    if abs_x <= 1.0 {
        ((A + 2.0) * abs_x - (A + 3.0)) * abs_x * abs_x + 1.0
    } else if abs_x < 2.0 {
        ((A * abs_x - 5.0 * A) * abs_x + 8.0 * A) * abs_x - 4.0 * A
    } else {
        0.0
    }
}

/// Sample a pixel with bounds checking. Returns `border_value` for out-of-bounds coordinates.
#[inline]
pub(super) fn sample_pixel(
    data: &[f32],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    border_value: f32,
) -> f32 {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        border_value
    } else {
        data[y as usize * width + x as usize]
    }
}

#[inline]
fn interpolate_nearest(data: &Buffer2<f32>, x: f32, y: f32, border_value: f32) -> f32 {
    sample_pixel(
        data.pixels(),
        data.width(),
        data.height(),
        x.round() as i32,
        y.round() as i32,
        border_value,
    )
}

#[inline]
fn interpolate_bilinear(data: &Buffer2<f32>, x: f32, y: f32, border_value: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let (pixels, w, h) = (data.pixels(), data.width(), data.height());
    let p00 = sample_pixel(pixels, w, h, x0, y0, border_value);
    let p10 = sample_pixel(pixels, w, h, x0 + 1, y0, border_value);
    let p01 = sample_pixel(pixels, w, h, x0, y0 + 1, border_value);
    let p11 = sample_pixel(pixels, w, h, x0 + 1, y0 + 1, border_value);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);
    top + fy * (bottom - top)
}

fn interpolate_bicubic(data: &Buffer2<f32>, x: f32, y: f32, border_value: f32) -> f32 {
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

    let (pixels, w, h) = (data.pixels(), data.width(), data.height());
    let mut sum = 0.0;
    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - 1 + j as i32;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - 1 + i as i32;
            sum += sample_pixel(pixels, w, h, px, py, border_value) * wxi * wyj;
        }
    }
    sum
}

#[inline]
fn interpolate_lanczos(data: &Buffer2<f32>, x: f32, y: f32, a: usize, border_value: f32) -> f32 {
    match a {
        2 => interpolate_lanczos_impl::<2, 4>(data, x, y, border_value),
        3 => interpolate_lanczos_impl::<3, 6>(data, x, y, border_value),
        4 => interpolate_lanczos_impl::<4, 8>(data, x, y, border_value),
        _ => panic!("Unsupported Lanczos parameter: {a}"),
    }
}

#[inline]
fn interpolate_lanczos_impl<const A: usize, const SIZE: usize>(
    data: &Buffer2<f32>,
    x: f32,
    y: f32,
    border_value: f32,
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
        *w = lut.lookup(fx - (i as i32 - a_i32 + 1) as f32);
        wx_sum += *w;
    }
    for (j, w) in wy.iter_mut().enumerate() {
        *w = lut.lookup(fy - (j as i32 - a_i32 + 1) as f32);
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

    let (pixels, w, h) = (data.pixels(), data.width(), data.height());
    let mut sum = 0.0f32;
    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - a_i32 + 1 + j as i32;
        let wyj = wyj * inv_wy;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - a_i32 + 1 + i as i32;
            sum += sample_pixel(pixels, w, h, px, py, border_value) * wxi * inv_wx * wyj;
        }
    }
    sum
}

#[inline]
fn interpolate(data: &Buffer2<f32>, x: f32, y: f32, params: &WarpParams) -> f32 {
    match params.method {
        InterpolationMethod::Nearest => interpolate_nearest(data, x, y, params.border_value),
        InterpolationMethod::Bilinear => interpolate_bilinear(data, x, y, params.border_value),
        InterpolationMethod::Bicubic => interpolate_bicubic(data, x, y, params.border_value),
        InterpolationMethod::Lanczos2 { .. } => {
            interpolate_lanczos(data, x, y, 2, params.border_value)
        }
        InterpolationMethod::Lanczos3 { .. } => {
            interpolate_lanczos(data, x, y, 3, params.border_value)
        }
        InterpolationMethod::Lanczos4 { .. } => {
            interpolate_lanczos(data, x, y, 4, params.border_value)
        }
    }
}

/// Warp an image using a [`WarpTransform`] (linear transform + optional SIP correction).
///
/// For each output pixel `p`, computes `src = warp_transform.apply(p)` and samples
/// the input image at `src`. Parallelized with rayon. Bilinear and Lanczos3 use
/// optimized row-warping paths.
pub fn warp_image(
    input: &Buffer2<f32>,
    output: &mut Buffer2<f32>,
    warp_transform: &WarpTransform,
    params: &WarpParams,
) {
    let width = input.width();
    let height = input.height();
    debug_assert_eq!(width, output.width());
    debug_assert_eq!(height, output.height());

    output
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            if params.method == InterpolationMethod::Bilinear {
                warp::warp_row_bilinear(input, row, y, warp_transform, params.border_value);
            } else if matches!(params.method, InterpolationMethod::Lanczos3 { .. }) {
                warp::warp_row_lanczos3(input, row, y, warp_transform, params);
            } else {
                for (x, pixel) in row.iter_mut().enumerate() {
                    let p = glam::DVec2::new(x as f64, y as f64);
                    let src = warp_transform.apply(p);
                    *pixel = interpolate(input, src.x as f32, src.y as f32, params);
                }
            }
        });
}
