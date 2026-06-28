//! Image interpolation for sub-pixel resampling.
//!
//! Provides Lanczos, bicubic, bilinear, and nearest-neighbor interpolation.
//! Bilinear and Lanczos3 have optimized row-warping paths (AVX2/SSE4.1 on x86_64,
//! scalar with incremental stepping on aarch64).

use std::f32::consts::PI;
use std::sync::OnceLock;

use rayon::prelude::*;

use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::transform::WarpTransform;
use common::Vec2us;
use glam::{IVec2, Vec2};
use imaginarium::Buffer2;

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
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let a_i32 = A as i32;
    let lut = get_lanczos_lut(A);

    let mut wx = [0.0f32; SIZE];
    let mut wy = [0.0f32; SIZE];
    for (i, w) in wx.iter_mut().enumerate() {
        *w = lut.lookup(fx - (i as i32 - a_i32 + 1) as f32);
    }
    for (j, w) in wy.iter_mut().enumerate() {
        *w = lut.lookup(fy - (j as i32 - a_i32 + 1) as f32);
    }

    let (pixels, w, h) = (data.pixels(), data.width(), data.height());
    let (wi, hi) = (w as i32, h as i32);
    // Drop out-of-bounds taps and divide by the in-bounds weight (the in-bounds weighted
    // average). Interior pixels are unchanged: `w_in` equals the full kernel sum that the
    // old `inv_wx * inv_wy` normalization divided by.
    let mut sum = 0.0f32;
    let mut w_in = 0.0f32;
    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - a_i32 + 1 + j as i32;
        if py < 0 || py >= hi {
            continue;
        }
        let row_off = py as usize * w;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - a_i32 + 1 + i as i32;
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

/// Per-pixel sampler for any method — the reference oracle the optimized row paths
/// ([`warp_image`]) are validated against. Production samples Nearest/Bicubic directly and uses the
/// SIMD row paths for Bilinear/Lanczos, so this dispatcher itself is test-only.
#[cfg(test)]
fn interpolate(data: &Buffer2<f32>, pos: Vec2, params: &WarpParams) -> f32 {
    match params.method {
        InterpolationMethod::Nearest => interpolate_nearest(data, pos, params.border_value),
        InterpolationMethod::Bilinear => bilinear_sample(data, pos, params.border_value),
        InterpolationMethod::Bicubic => interpolate_bicubic(data, pos, params.border_value),
        InterpolationMethod::Lanczos2 { .. } => {
            interpolate_lanczos(data, pos, 2, params.border_value)
        }
        InterpolationMethod::Lanczos3 { .. } => {
            interpolate_lanczos(data, pos, 3, params.border_value)
        }
        InterpolationMethod::Lanczos4 { .. } => {
            interpolate_lanczos(data, pos, 4, params.border_value)
        }
    }
}

/// Fraction of the interpolation kernel's weight at one output pixel that lands
/// on in-bounds source samples — `Σ_in(w) / Σ_all(w) ∈ [0, 1]`. Pure geometry:
/// it mirrors each sampler's tap layout + weights but reads no pixel data, so
/// it needs neither a scratch source buffer nor a second sampling pass. The
/// `1.0` for fully-interior pixels and `0.0` for fully-extrapolated ones is the
/// per-pixel data-fraction weight [`warp_coverage`] feeds to downstream stacking.
/// The warped *value* is renormalized to the in-bounds weighted average, but *where*
/// depends on the kernel: bicubic/Lanczos (negative lobes) renormalize inside the
/// sampler, while nearest/bilinear keep raw border-blended values that
/// `registration::warp` divides by this coverage afterward (`renormalize_by_coverage`).
/// Either way the value is already an average, so it is not divided by coverage again.
fn coverage_at(pos: Vec2, dims: Vec2us, method: InterpolationMethod) -> f32 {
    let (sx, sy) = (pos.x, pos.y);
    let in_bounds =
        |c: IVec2| c.x >= 0 && c.y >= 0 && (c.x as usize) < dims.x && (c.y as usize) < dims.y;
    match method {
        InterpolationMethod::Nearest => {
            if in_bounds(IVec2::new(sx.round() as i32, sy.round() as i32)) {
                1.0
            } else {
                0.0
            }
        }
        InterpolationMethod::Bilinear => {
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            separable_in_bounds_fraction(x0, &[1.0 - fx, fx], y0, &[1.0 - fy, fy], dims)
        }
        InterpolationMethod::Bicubic => {
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            let wx = bicubic_weights(fx);
            let wy = bicubic_weights(fy);
            separable_in_bounds_fraction(x0 - 1, &wx, y0 - 1, &wy, dims)
        }
        InterpolationMethod::Lanczos2 { .. }
        | InterpolationMethod::Lanczos3 { .. }
        | InterpolationMethod::Lanczos4 { .. } => {
            let a = match method {
                InterpolationMethod::Lanczos2 { .. } => 2,
                InterpolationMethod::Lanczos3 { .. } => 3,
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
            separable_in_bounds_fraction(x0 - ai + 1, &wx[..size], y0 - ai + 1, &wy[..size], dims)
        }
    }
}

/// Sum the in-bounds tap weights of a separable kernel and divide by the total
/// weight — the fraction of the kernel that landed on real samples. The samplers
/// themselves divide their in-bounds value sum by the in-bounds weight, so this is
/// the downstream stacking weight, not a value-renormalization factor.
fn separable_in_bounds_fraction(x0: i32, wx: &[f32], y0: i32, wy: &[f32], dims: Vec2us) -> f32 {
    let wsum = wx.iter().sum::<f32>() * wy.iter().sum::<f32>();
    if wsum.abs() < 1e-10 {
        return 0.0;
    }
    let mut in_sum = 0.0f32;
    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 + j as i32;
        if py < 0 || py as usize >= dims.y {
            continue;
        }
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 + i as i32;
            if px >= 0 && (px as usize) < dims.x {
                in_sum += wxi * wyj;
            }
        }
    }
    (in_sum / wsum).clamp(0.0, 1.0)
}

/// Per-pixel coverage map for a warp — the geometric companion to
/// [`warp_image`], computed in the same inverse-mapped row order with the same
/// incremental coordinate stepping but reading no pixel data (see
/// [`coverage_at`]). Channel-independent, so the caller computes it once.
pub(crate) fn warp_coverage(
    dims: Vec2us,
    wt: &WarpTransform,
    method: InterpolationMethod,
) -> Buffer2<f32> {
    let mut coverage = Buffer2::new_default(dims.x, dims.y);
    coverage
        .pixels_mut()
        .par_chunks_mut(dims.x)
        .enumerate()
        .for_each(|(y, row)| {
            warp::warp_row_with(y, wt, row, |pos| coverage_at(pos, dims, method));
        });
    coverage
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
                        warp::warp_row_with(y, warp_transform, row, |pos| {
                            interpolate_bicubic(input, pos, border)
                        })
                    }
                    _ => warp::warp_row_with(y, warp_transform, row, |pos| {
                        interpolate_nearest(input, pos, border)
                    }),
                }
            }
        });
}

#[cfg(test)]
impl WarpParams {
    pub(crate) fn new(method: InterpolationMethod) -> Self {
        Self {
            method,
            ..Default::default()
        }
    }
}
