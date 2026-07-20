//! Image interpolation for sub-pixel resampling.
//!
//! Provides Lanczos, bicubic, bilinear, and nearest-neighbor interpolation.
//! Bilinear and Lanczos3 have optimized row-warping paths (AVX2/SSE4.1 on x86_64,
//! scalar with incremental stepping on aarch64).

use std::f32::consts::PI;
use std::sync::OnceLock;

use rayon::prelude::*;

use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::result::RegistrationError;
use crate::stacking::registration::transform::WarpTransform;
use common::Vec2us;
use glam::{IVec2, Vec2};
use imaginarium::Buffer2;

/// Bundled warp parameters passed through the interpolation pipeline.
#[derive(Debug, Clone, Copy)]
pub struct WarpParams {
    /// Resampling kernel.
    pub method: InterpolationMethod,
    /// Constant value sampled outside the input image.
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
    pub(crate) fn validate(&self) -> Result<(), RegistrationError> {
        if !self.border_value.is_finite() {
            return Err(RegistrationError::InvalidConfig(format!(
                "warp border_value must be finite, got {}",
                self.border_value
            )));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn new(method: InterpolationMethod) -> Self {
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

pub(crate) mod warp;

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
pub(crate) struct LanczosLut {
    pub(crate) values: Vec<f32>,
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
    pub(crate) fn lookup(&self, x: f32) -> f32 {
        let abs_x = x.abs();
        if abs_x >= self.a as f32 {
            return 0.0;
        }
        let idx = (abs_x * LANCZOS_LUT_RESOLUTION as f32 + 0.5) as usize;
        unsafe { *self.values.get_unchecked(idx) }
    }

    /// Fast lookup for known non-negative distance within [0, a].
    ///
    /// Skips the `abs()` and `>= a` branch. The caller must guarantee that
    /// `abs_x` is in `[0, a]`. Used in the Lanczos3 inner loop where fractional
    /// parts are computed such that all distances are known-positive.
    #[inline(always)]
    pub(crate) fn lookup_positive(&self, abs_x: f32) -> f32 {
        debug_assert!(abs_x >= 0.0 && abs_x <= self.a as f32);
        let idx = (abs_x * LANCZOS_LUT_RESOLUTION as f32 + 0.5) as usize;
        unsafe { *self.values.get_unchecked(idx) }
    }
}

static LANCZOS2_LUT: OnceLock<LanczosLut> = OnceLock::new();
static LANCZOS3_LUT: OnceLock<LanczosLut> = OnceLock::new();
static LANCZOS4_LUT: OnceLock<LanczosLut> = OnceLock::new();

#[inline]
pub(crate) fn get_lanczos_lut(a: usize) -> &'static LanczosLut {
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

/// The four 1-D bicubic tap weights for a fractional offset `f`, ordered for
/// taps at `floor - 1 ..= floor + 2`. Single source of truth shared by the
/// bicubic sampler and the coverage pass.
#[inline]
fn bicubic_weights(f: f32) -> [f32; 4] {
    [
        bicubic_kernel(f + 1.0),
        bicubic_kernel(f),
        bicubic_kernel(f - 1.0),
        bicubic_kernel(f - 2.0),
    ]
}

/// Sample a pixel with bounds checking. Returns `border_value` for out-of-bounds coordinates.
#[inline]
pub(crate) fn sample_pixel(data: &[f32], dims: Vec2us, coord: IVec2, border_value: f32) -> f32 {
    if coord.x < 0 || coord.y < 0 || coord.x >= dims.x as i32 || coord.y >= dims.y as i32 {
        border_value
    } else {
        data[coord.y as usize * dims.x + coord.x as usize]
    }
}

/// Fast inline floor-to-i32, avoiding a libc `floorf` call.
///
/// Truncates toward zero then corrects for negatives: for `x = -0.5`, `i = 0`, `x < 0.0` so result
/// `-1`. Correct for the finite source coordinates the warp paths produce.
#[inline(always)]
pub(crate) fn fast_floor_i32(x: f32) -> i32 {
    let i = x as i32;
    i - (x < i as f32) as i32
}

/// Bilinear sample at a single point. The per-pixel primitive shared by the scalar bilinear row path,
/// the SIMD bilinear backends' border/tail handling, and the test oracle.
#[inline]
pub(crate) fn bilinear_sample(input: &Buffer2<f32>, pos: Vec2, border_value: f32) -> f32 {
    let (x, y) = (pos.x, pos.y);
    let pixels = input.pixels();
    let dims = Vec2us::new(input.width(), input.height());

    let x0 = fast_floor_i32(x);
    let y0 = fast_floor_i32(y);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = sample_pixel(pixels, dims, IVec2::new(x0, y0), border_value);
    let p10 = sample_pixel(pixels, dims, IVec2::new(x0 + 1, y0), border_value);
    let p01 = sample_pixel(pixels, dims, IVec2::new(x0, y0 + 1), border_value);
    let p11 = sample_pixel(pixels, dims, IVec2::new(x0 + 1, y0 + 1), border_value);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);
    top + fy * (bottom - top)
}

#[inline]
pub(crate) fn bilinear_sample_edge_extended(input: &Buffer2<f32>, pos: Vec2) -> f32 {
    let max = Vec2::new(
        input.width().saturating_sub(1) as f32,
        input.height().saturating_sub(1) as f32,
    );
    bilinear_sample(input, pos.clamp(Vec2::ZERO, max), 0.0)
}

#[inline]
fn interpolate_nearest(data: &Buffer2<f32>, pos: Vec2, border_value: f32) -> f32 {
    sample_pixel(
        data.pixels(),
        Vec2us::new(data.width(), data.height()),
        IVec2::new(pos.x.round() as i32, pos.y.round() as i32),
        border_value,
    )
}

fn interpolate_bicubic(data: &Buffer2<f32>, pos: Vec2, border_value: f32) -> f32 {
    let (x, y) = (pos.x, pos.y);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let wx = bicubic_weights(fx);
    let wy = bicubic_weights(fy);

    let (pixels, w, h) = (data.pixels(), data.width(), data.height());
    let (wi, hi) = (w as i32, h as i32);
    // Drop out-of-bounds taps and divide by the in-bounds weight so edge pixels get the
    // true in-bounds weighted average instead of being darkened by missing taps. Interior
    // pixels are unchanged: bicubic weights sum to 1, so `w_in == 1`.
    let mut sum = 0.0f32;
    let mut w_in = 0.0f32;
    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - 1 + j as i32;
        if py < 0 || py >= hi {
            continue;
        }
        let row_off = py as usize * w;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - 1 + i as i32;
            if px < 0 || px >= wi {
                continue;
            }
            let weight = wxi * wyj;
            sum += pixels[row_off + px as usize] * weight;
            w_in += weight;
        }
    }
    if w_in.abs() < 1e-10 {
        border_value
    } else {
        sum / w_in
    }
}

/// Per-pixel Lanczos sampler. The production warp uses the optimized row path
/// ([`warp::warp_row_lanczos`]); this is the readable reference the row path is validated against.
#[cfg(test)]
#[inline]
fn interpolate_lanczos(data: &Buffer2<f32>, pos: Vec2, a: usize, border_value: f32) -> f32 {
    match a {
        2 => interpolate_lanczos_impl::<2, 4>(data, pos, border_value),
        3 => interpolate_lanczos_impl::<3, 6>(data, pos, border_value),
        4 => interpolate_lanczos_impl::<4, 8>(data, pos, border_value),
        _ => panic!("Unsupported Lanczos parameter: {a}"),
    }
}

#[cfg(test)]
#[inline]
fn interpolate_lanczos_impl<const A: usize, const SIZE: usize>(
    data: &Buffer2<f32>,
    pos: Vec2,
    border_value: f32,
) -> f32 {
    let (x, y) = (pos.x, pos.y);
    let (w, h) = (data.width(), data.height());
    let support_min = -(A as f32);
    let support_max = Vec2::new(w as f32 + A as f32 - 1.0, h as f32 + A as f32 - 1.0);
    if !pos.is_finite()
        || x < support_min
        || y < support_min
        || x >= support_max.x
        || y >= support_max.y
    {
        return border_value;
    }

    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let a_i32 = A as i32;
    let kx0 = x0 - a_i32 + 1;
    let ky0 = y0 - a_i32 + 1;
    if kx0 < 0 || ky0 < 0 || kx0 + SIZE as i32 > w as i32 || ky0 + SIZE as i32 > h as i32 {
        return bilinear_sample_edge_extended(data, pos);
    }

    let lut = get_lanczos_lut(A);

    let mut wx = [0.0f32; SIZE];
    let mut wy = [0.0f32; SIZE];
    for (i, w) in wx.iter_mut().enumerate() {
        *w = lut.lookup(fx - (i as i32 - a_i32 + 1) as f32);
    }
    for (j, w) in wy.iter_mut().enumerate() {
        *w = lut.lookup(fy - (j as i32 - a_i32 + 1) as f32);
    }

    let pixels = data.pixels();
    let mut sum = 0.0f32;
    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - a_i32 + 1 + j as i32;
        let row_off = py as usize * w;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - a_i32 + 1 + i as i32;
            let weight = wxi * wyj;
            sum += pixels[row_off + px as usize] * weight;
        }
    }
    let total_weight = wx.iter().sum::<f32>() * wy.iter().sum::<f32>();
    if total_weight.abs() < 1e-10 {
        sum
    } else {
        sum / total_weight
    }
}

/// Per-pixel sampler for any method — the reference oracle the optimized row paths
/// ([`warp_image`]) are validated against. Production samples Nearest/Bicubic directly and uses the
/// SIMD row paths for Bilinear/Lanczos, so this dispatcher itself is test-only.
#[cfg(test)]
fn interpolate(data: &Buffer2<f32>, pos: Vec2, params: &WarpParams) -> f32 {
    match params.method {
        InterpolationMethod::Nearest => interpolate_nearest(data, pos, params.border_value),
        InterpolationMethod::Bilinear => bilinear_sample(data, pos, params.border_value),
        InterpolationMethod::Bicubic => interpolate_bicubic(data, pos, params.border_value),
        InterpolationMethod::Lanczos2 => interpolate_lanczos(data, pos, 2, params.border_value),
        InterpolationMethod::Lanczos3 => interpolate_lanczos(data, pos, 3, params.border_value),
        InterpolationMethod::Lanczos4 => interpolate_lanczos(data, pos, 4, params.border_value),
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct AxisWeightStats {
    signed: f32,
    magnitude: f32,
    square: f32,
    in_signed: f32,
    in_magnitude: f32,
    in_square: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct SampleQuality {
    coverage: f32,
    confidence: f32,
    normalization: f32,
}

#[derive(Debug)]
pub(crate) struct WarpQualityMaps {
    pub(crate) coverage: Buffer2<f32>,
    pub(crate) confidence: Buffer2<f32>,
    pub(crate) normalization: Option<Buffer2<f32>>,
}

fn axis_weight_stats(start: i32, weights: &[f32], length: usize) -> AxisWeightStats {
    let mut stats = AxisWeightStats::default();
    for (i, &weight) in weights.iter().enumerate() {
        let magnitude = weight.abs();
        let square = weight * weight;
        stats.signed += weight;
        stats.magnitude += magnitude;
        stats.square += square;
        let coordinate = start + i as i32;
        if coordinate >= 0 && (coordinate as usize) < length {
            stats.in_signed += weight;
            stats.in_magnitude += magnitude;
            stats.in_square += square;
        }
    }
    stats
}

fn separable_coverage(x: AxisWeightStats, y: AxisWeightStats) -> f32 {
    let total = x.magnitude * y.magnitude;
    if total <= f32::EPSILON {
        0.0
    } else {
        ((x.in_magnitude * y.in_magnitude) / total).clamp(0.0, 1.0)
    }
}

fn separable_confidence(x: AxisWeightStats, y: AxisWeightStats) -> f32 {
    let normalization = x.in_signed * y.in_signed;
    let square = x.in_square * y.in_square;
    if normalization.abs() <= 1e-10 || square <= f32::EPSILON {
        0.0
    } else {
        normalization * normalization / square
    }
}

fn bilinear_quality(pos: Vec2, dims: Vec2us) -> SampleQuality {
    let (sx, sy) = (pos.x, pos.y);
    let x0 = sx.floor() as i32;
    let y0 = sy.floor() as i32;
    let fx = sx - x0 as f32;
    let fy = sy - y0 as f32;
    let x = axis_weight_stats(x0, &[1.0 - fx, fx], dims.x);
    let y = axis_weight_stats(y0, &[1.0 - fy, fy], dims.y);
    SampleQuality {
        coverage: separable_coverage(x, y),
        confidence: separable_confidence(x, y),
        normalization: x.in_signed * y.in_signed,
    }
}

fn quality_at(pos: Vec2, dims: Vec2us, method: InterpolationMethod) -> SampleQuality {
    if dims.x == 0 || dims.y == 0 {
        return SampleQuality::default();
    }
    let (sx, sy) = (pos.x, pos.y);
    let radius = method.kernel_radius() as f32;
    if sx < -radius || sy < -radius || sx >= dims.x as f32 + radius || sy >= dims.y as f32 + radius
    {
        return SampleQuality::default();
    }
    match method {
        InterpolationMethod::Nearest => {
            let coordinate = IVec2::new(sx.round() as i32, sy.round() as i32);
            if coordinate.x >= 0
                && coordinate.y >= 0
                && (coordinate.x as usize) < dims.x
                && (coordinate.y as usize) < dims.y
            {
                SampleQuality {
                    coverage: 1.0,
                    confidence: 1.0,
                    normalization: 1.0,
                }
            } else {
                SampleQuality::default()
            }
        }
        InterpolationMethod::Bilinear => bilinear_quality(pos, dims),
        InterpolationMethod::Bicubic => {
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            let wx = bicubic_weights(fx);
            let wy = bicubic_weights(fy);
            let x = axis_weight_stats(x0 - 1, &wx, dims.x);
            let y = axis_weight_stats(y0 - 1, &wy, dims.y);
            SampleQuality {
                coverage: separable_coverage(x, y),
                confidence: separable_confidence(x, y),
                normalization: 1.0,
            }
        }
        InterpolationMethod::Lanczos2
        | InterpolationMethod::Lanczos3
        | InterpolationMethod::Lanczos4 => {
            let a = match method {
                InterpolationMethod::Lanczos2 => 2,
                InterpolationMethod::Lanczos3 => 3,
                _ => 4,
            };
            let lut = get_lanczos_lut(a);
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            let ai = a as i32;
            let size = 2 * a;
            // Max kernel is Lanczos4 (8 taps per axis).
            let mut wx = [0.0f32; 8];
            let mut wy = [0.0f32; 8];
            for i in 0..size {
                wx[i] = lut.lookup(fx - (i as i32 - ai + 1) as f32);
                wy[i] = lut.lookup(fy - (i as i32 - ai + 1) as f32);
            }
            let start_x = x0 - ai + 1;
            let start_y = y0 - ai + 1;
            let x = axis_weight_stats(start_x, &wx[..size], dims.x);
            let y = axis_weight_stats(start_y, &wy[..size], dims.y);
            let coverage = separable_coverage(x, y);
            let fully_supported = start_x >= 0
                && start_y >= 0
                && start_x + size as i32 <= dims.x as i32
                && start_y + size as i32 <= dims.y as i32;
            let confidence = if coverage == 0.0 {
                0.0
            } else if fully_supported {
                separable_confidence(x, y)
            } else {
                let max = Vec2::new(
                    dims.x.saturating_sub(1) as f32,
                    dims.y.saturating_sub(1) as f32,
                );
                bilinear_quality(pos.clamp(Vec2::ZERO, max), dims).confidence
            };
            SampleQuality {
                coverage,
                confidence,
                normalization: 1.0,
            }
        }
    }
}

/// Geometric support and interpolation confidence for one warp.
pub(crate) fn warp_quality_maps(
    dims: Vec2us,
    wt: &WarpTransform,
    method: InterpolationMethod,
) -> WarpQualityMaps {
    let mut coverage = Buffer2::new_default(dims.x, dims.y);
    let mut confidence = Buffer2::new_default(dims.x, dims.y);
    let normalization = if method == InterpolationMethod::Bilinear {
        let mut normalization = Buffer2::new_default(dims.x, dims.y);
        coverage
            .pixels_mut()
            .par_chunks_mut(dims.x)
            .zip(confidence.pixels_mut().par_chunks_mut(dims.x))
            .zip(normalization.pixels_mut().par_chunks_mut(dims.x))
            .enumerate()
            .for_each(|(y, ((coverage_row, confidence_row), normalization_row))| {
                warp::for_each_source_position(y, wt, dims.x, |x, pos| {
                    let quality = pos
                        .map_or_else(SampleQuality::default, |pos| quality_at(pos, dims, method));
                    coverage_row[x] = quality.coverage;
                    confidence_row[x] = quality.confidence;
                    normalization_row[x] = quality.normalization;
                });
            });
        Some(normalization)
    } else {
        coverage
            .pixels_mut()
            .par_chunks_mut(dims.x)
            .zip(confidence.pixels_mut().par_chunks_mut(dims.x))
            .enumerate()
            .for_each(|(y, (coverage_row, confidence_row))| {
                warp::for_each_source_position(y, wt, dims.x, |x, pos| {
                    let quality = pos
                        .map_or_else(SampleQuality::default, |pos| quality_at(pos, dims, method));
                    coverage_row[x] = quality.coverage;
                    confidence_row[x] = quality.confidence;
                });
            });
        None
    };

    WarpQualityMaps {
        coverage,
        confidence,
        normalization,
    }
}

/// Warp an image using a [`WarpTransform`] (linear transform + optional SIP correction).
///
/// For each output pixel `p`, computes `src = warp_transform.apply(p)` and samples
/// the input image at `src`. Parallelized with rayon. Bilinear and Lanczos3 use
/// optimized row-warping paths.
pub(crate) fn warp_image(
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
            } else if params.method.lanczos_param().is_some() {
                warp::warp_row_lanczos(input, row, y, warp_transform, params);
            } else {
                // Nearest / Bicubic have no dedicated row path. Dispatch once per row (not per pixel)
                // and sample point-by-point; Bilinear and Lanczos are handled above.
                let border = params.border_value;
                match params.method {
                    InterpolationMethod::Bicubic => {
                        warp::warp_row_with(y, warp_transform, row, border, |pos| {
                            interpolate_bicubic(input, pos, border)
                        })
                    }
                    _ => warp::warp_row_with(y, warp_transform, row, border, |pos| {
                        interpolate_nearest(input, pos, border)
                    }),
                }
            }
        });
}
