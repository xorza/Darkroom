//! Non-linear **stretching**: map a linear, stacked image — faint signal sitting just above a
//! near-black background, stars 10⁴–10⁵× brighter — to a display image with strong contrast
//! across that range. The curve is steep near the background (expanding the thin slice of nebula
//! signal) and shallow in the highlights (compressing star cores so they don't saturate).
//!
//! Two algorithms (see `docs/image-stretching.md` for the full derivation):
//! - **STF / MTF auto-stretch** (PixInsight/Siril): a linear black-point clip-rescale followed by
//!   the Midtones Transfer Function `MTF(m,x) = (m−1)x / ((2m−1)x − m)` — a rational (Möbius)
//!   curve, *not* a gamma curve. The black point (`median − k·σ`) and midtones `m` are derived
//!   from the image median and MAD. Fully automatic; the standard "screen stretch".
//! - **Normalized arcsinh** (Lupton et al. 2004): `f(x) = asinh(x/β) / asinh(1/β)`, linear near
//!   black (faint detail, low noise gain) and logarithmic in the highlights (compressed cores),
//!   with `β` chosen automatically from the background level.
//!
//! Both default to **color-preserving** application: the curve runs on the combined intensity
//! `I = (r+g+b)/3` and every channel is scaled by `f(I)/I`, so hue/saturation and star color are
//! preserved and only intensity is remapped. A per-channel stretch instead ties an object's color
//! to its brightness and burns bright star cores toward white.

use rayon::prelude::*;

use crate::core::math::statistics::{mad_to_sigma, median_and_mad_f32_mut};
use crate::io::astro_image::{AstroImage, Rgb};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod real_data_tests;

/// Midtones balance is clamped away from the degenerate endpoints `0`/`1`, where the MTF
/// collapses every interior value onto a single output.
const MIDTONES_MIN: f32 = 1e-4;
const MIDTONES_MAX: f32 = 1.0 - 1e-4;

/// Which stretch curve to apply, and how its parameters are chosen.
#[derive(Debug, Clone, Copy)]
pub enum StretchMethod {
    /// Screen-Transfer-Function (MTF) auto-stretch. Black point `= median − shadow_sigmas·σ`
    /// (σ from MAD), and the midtones balance is chosen so the rescaled median lands on
    /// `target_background`.
    AutoStf {
        shadow_sigmas: f32,
        target_background: f32,
    },
    /// Normalized arcsinh with softening `β` chosen so the background median maps to
    /// `target_background`.
    AutoAsinh { target_background: f32 },
    /// Normalized arcsinh with an explicit softening `β` (smaller = stronger stretch).
    Asinh { beta: f32 },
}

/// How a stretch curve is applied across the channels of a color image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Stretch the combined intensity `I = (r+g+b)/3` and scale each channel by `f(I)/I`.
    /// Preserves hue/saturation and star color; recommended for a final image. Grayscale is
    /// stretched directly.
    ColorPreserving,
    /// Stretch each channel independently with its own auto parameters — auto-neutralizes the
    /// background toward gray but ties color to brightness. For a quick screen preview.
    PerChannel,
}

/// A stretch to apply to a stacked [`AstroImage`]. Pixels are expected in `[0, 1]` and stay in
/// `[0, 1]`.
#[derive(Debug, Clone, Copy)]
pub struct StretchConfig {
    pub method: StretchMethod,
    pub color: ColorMode,
}

impl StretchConfig {
    /// Color-preserving normalized-arcsinh auto-stretch — the recommended best-quality default.
    pub fn auto_asinh() -> Self {
        Self {
            method: StretchMethod::AutoAsinh {
                target_background: 0.2,
            },
            color: ColorMode::ColorPreserving,
        }
    }

    /// Color-preserving STF (MTF) auto-stretch — the standard automatic "screen stretch".
    pub fn auto_stf() -> Self {
        Self {
            method: StretchMethod::AutoStf {
                shadow_sigmas: 1.0,
                target_background: 0.25,
            },
            color: ColorMode::ColorPreserving,
        }
    }
}

impl Default for StretchConfig {
    fn default() -> Self {
        Self::auto_asinh()
    }
}

/// Apply a non-linear stretch to a stacked image in place.
pub fn stretch(image: &mut AstroImage, config: &StretchConfig) {
    match config.color {
        ColorMode::ColorPreserving => {
            let curve = build_curve_combined(image, &config.method);
            apply_color_preserving(image, curve);
        }
        ColorMode::PerChannel => {
            for c in 0..image.channels() {
                let mut samples = image.channel(c).to_vec();
                let curve = build_curve(&mut samples, &config.method);
                apply_to_slice(image.channel_mut(c).pixels_mut(), curve);
            }
        }
    }
}

/// A prepared monotonic tone curve `f: [0,1] → [0,1]` with its parameters precomputed.
#[derive(Debug, Clone, Copy)]
enum Curve {
    /// Linear clip-rescale `[black, 1] → [0, 1]`, then `MTF(midtones, ·)`.
    Stf {
        black: f32,
        inv_range: f32,
        midtones: f32,
    },
    /// Normalized arcsinh: `asinh(x · inv_beta) · inv_norm`.
    Asinh { inv_beta: f32, inv_norm: f32 },
}

impl Curve {
    fn stf(median: f32, sigma: f32, shadow_sigmas: f32, target_bkg: f32) -> Self {
        let black = (median - shadow_sigmas * sigma).clamp(0.0, 1.0);
        let inv_range = if black < 1.0 {
            1.0 / (1.0 - black)
        } else {
            1.0
        };
        let rescaled_median = ((median - black) * inv_range).clamp(0.0, 1.0);
        // MTF's Möbius self-inverse identity: MTF(MTF(t, x0), x0) = t, so the midtones balance
        // that maps the rescaled median onto the target background is just MTF(target, median).
        let midtones = mtf(target_bkg, rescaled_median).clamp(MIDTONES_MIN, MIDTONES_MAX);
        Curve::Stf {
            black,
            inv_range,
            midtones,
        }
    }

    fn asinh(beta: f32) -> Self {
        assert!(
            beta > 0.0,
            "asinh softening beta must be positive, got {beta}"
        );
        let inv_beta = 1.0 / beta;
        Curve::Asinh {
            inv_beta,
            inv_norm: 1.0 / inv_beta.asinh(),
        }
    }

    #[inline]
    fn eval(&self, x: f32) -> f32 {
        match *self {
            Curve::Stf {
                black,
                inv_range,
                midtones,
            } => {
                let v = ((x - black) * inv_range).clamp(0.0, 1.0);
                mtf(midtones, v)
            }
            Curve::Asinh { inv_beta, inv_norm } => (x * inv_beta).asinh() * inv_norm,
        }
    }
}

/// Midtones Transfer Function: a rational (Möbius) interpolation through `(0,0)`, `(m,0.5)`,
/// `(1,1)`. `m = 0.5` is the identity; `m < 0.5` brightens midtones. Not a gamma curve.
#[inline]
fn mtf(m: f32, x: f32) -> f32 {
    if x <= 0.0 {
        0.0
    } else if x >= 1.0 {
        1.0
    } else {
        ((m - 1.0) * x) / ((2.0 * m - 1.0) * x - m)
    }
}

/// Choose the arcsinh softening `β` so a background of `median` maps to `target_background`.
///
/// `g(β) = asinh(median/β) / asinh(1/β)` is monotonically decreasing in `β`, ranging from ~1 as
/// `β → 0` (strong, log-like) to `median` as `β → ∞` (near-linear). Bisect `log₁₀ β` to hit the
/// target, which must lie in `(median, 1)`.
fn solve_asinh_beta(median: f32, target_background: f32) -> f32 {
    let target = target_background.clamp(median + 1e-4, 1.0 - 1e-4);
    let (mut lo, mut hi) = (-5.0f32, 5.0f32);
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        let beta = 10.0f32.powf(mid);
        let g = (median / beta).asinh() / (1.0 / beta).asinh();
        if g > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    10.0f32.powf(0.5 * (lo + hi))
}

/// Build the curve for color-preserving mode from the combined-intensity statistics.
fn build_curve_combined(image: &AstroImage, method: &StretchMethod) -> Curve {
    if let StretchMethod::Asinh { beta } = *method {
        return Curve::asinh(beta); // explicit β needs no statistics
    }
    let mut plane = image.intensity_plane();
    build_curve(plane.pixels_mut(), method)
}

/// Build a curve from a sample set, computing statistics in place as needed.
fn build_curve(samples: &mut [f32], method: &StretchMethod) -> Curve {
    match *method {
        StretchMethod::AutoStf {
            shadow_sigmas,
            target_background,
        } => {
            let (median, mad) = median_and_mad_f32_mut(samples);
            Curve::stf(median, mad_to_sigma(mad), shadow_sigmas, target_background)
        }
        StretchMethod::AutoAsinh { target_background } => {
            let (median, _) = median_and_mad_f32_mut(samples);
            Curve::asinh(solve_asinh_beta(median, target_background))
        }
        StretchMethod::Asinh { beta } => Curve::asinh(beta),
    }
}

fn apply_to_slice(pixels: &mut [f32], curve: Curve) {
    pixels.par_iter_mut().for_each(|v| *v = curve.eval(*v));
}

/// Apply `curve` to the combined intensity and scale each channel by `f(I)/I`, with a
/// hue-preserving cap when a channel would exceed 1 (divide all three by their max).
fn apply_color_preserving(image: &mut AstroImage, curve: Curve) {
    image.par_map_pixels(
        |l| curve.eval(l),
        |px| {
            let intensity = px.intensity();
            if intensity <= 0.0 {
                return Rgb::ZERO;
            }
            let scaled = px.scale(curve.eval(intensity) / intensity);
            // Hue-preserving highlight cap: when a channel exceeds 1, divide all three by the max.
            let maxc = scaled.r.max(scaled.g).max(scaled.b);
            if maxc > 1.0 {
                scaled.scale(1.0 / maxc)
            } else {
                scaled
            }
        },
    );
}
