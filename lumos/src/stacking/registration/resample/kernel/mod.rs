//! Image interpolation for sub-pixel resampling.
//!
//! Provides Lanczos, bicubic, bilinear, and nearest-neighbor interpolation.
//! Bilinear and Lanczos3 have optimized row-warping paths (AVX2/SSE4.1 on x86_64,
//! scalar with incremental stepping on aarch64).

use std::f32::consts::PI;
use std::sync::OnceLock;

use common::Vec2us;
use glam::Vec2;
use imaginarium::Buffer2;

#[cfg(test)]
mod tests;

// Lanczos LUT: 4096 samples/unit gives ~0.00024 precision.
// Lanczos3 LUT: 4096 * 3 * 4 bytes = 48KB (fits in L1 cache).
pub(crate) const LANCZOS_LUT_RESOLUTION: usize = 4096;

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
pub(crate) fn bicubic_weights(f: f32) -> [f32; 4] {
    [
        bicubic_kernel(f + 1.0),
        bicubic_kernel(f),
        bicubic_kernel(f - 1.0),
        bicubic_kernel(f - 2.0),
    ]
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

#[inline]
pub(crate) fn source_footprint_contains(pos: Vec2, dims: Vec2us) -> bool {
    if dims.x == 0 || dims.y == 0 || !pos.is_finite() {
        return false;
    }
    let max = Vec2::new(dims.x as f32 - 0.5, dims.y as f32 - 0.5);
    pos.x >= -0.5 && pos.y >= -0.5 && pos.x <= max.x && pos.y <= max.y
}

#[inline]
pub(crate) fn clamp_to_pixel_centers(pos: Vec2, dims: Vec2us) -> Vec2 {
    debug_assert!(dims.x > 0 && dims.y > 0);
    pos.clamp(
        Vec2::ZERO,
        Vec2::new(dims.x as f32 - 1.0, dims.y as f32 - 1.0),
    )
}

/// Bilinear sample at a single point. The per-pixel primitive shared by the scalar bilinear row path,
/// the SIMD bilinear backends' border/tail handling, and the test oracle.
#[inline]
pub(crate) fn bilinear_sample(input: &Buffer2<f32>, pos: Vec2, border_value: f32) -> f32 {
    let pixels = input.pixels();
    let dims = Vec2us::new(input.width(), input.height());
    if !source_footprint_contains(pos, dims) {
        return border_value;
    }
    let sample_pos = clamp_to_pixel_centers(pos, dims);
    let (x, y) = (sample_pos.x, sample_pos.y);

    let x0 = fast_floor_i32(x) as usize;
    let y0 = fast_floor_i32(y) as usize;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let x1 = (x0 + 1).min(dims.x - 1);
    let y1 = (y0 + 1).min(dims.y - 1);

    let p00 = pixels[y0 * dims.x + x0];
    let p10 = pixels[y0 * dims.x + x1];
    let p01 = pixels[y1 * dims.x + x0];
    let p11 = pixels[y1 * dims.x + x1];

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);
    top + fy * (bottom - top)
}

#[inline]
pub(crate) fn interpolate_nearest(data: &Buffer2<f32>, pos: Vec2, border_value: f32) -> f32 {
    let dims = Vec2us::new(data.width(), data.height());
    if !source_footprint_contains(pos, dims) {
        return border_value;
    }
    let sample_pos = clamp_to_pixel_centers(pos, dims);
    let x = sample_pos.x.round() as usize;
    let y = sample_pos.y.round() as usize;
    data.pixels()[y * dims.x + x]
}

pub(crate) fn interpolate_bicubic(data: &Buffer2<f32>, pos: Vec2, border_value: f32) -> f32 {
    let dims = Vec2us::new(data.width(), data.height());
    if !source_footprint_contains(pos, dims) {
        return border_value;
    }
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

#[cfg(test)]
pub(crate) mod test_support;
