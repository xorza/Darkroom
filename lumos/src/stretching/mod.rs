//! Non-linear **stretching**: map a linear, stacked image — faint signal sitting just above a
//! near-black background, stars 10⁴–10⁵× brighter — to a display image with strong contrast
//! across that range. The curve is steep near the background (expanding the thin slice of nebula
//! signal) and shallow in the highlights (compressing star cores so they don't saturate).
//!
//! Input is treated as linear and non-negative; it need not be normalized (a raw stack's bright
//! stars routinely exceed 1). Every curve clamps its output, so the result is always a valid
//! display image in `[0, 1]`.
//!
//! Three curve families (see `docs/image-stretching.md` and `ghs.md` for the full derivations):
//! - **STF / MTF auto-stretch** (PixInsight/Siril): a linear black-point clip-rescale followed by
//!   the Midtones Transfer Function `MTF(m,x) = (m−1)x / ((2m−1)x − m)` — a rational (Möbius)
//!   curve, *not* a gamma curve. The black point (`median − k·σ`) and midtones `m` are derived
//!   from the image median and MAD. Fully automatic; the standard "screen stretch".
//! - **Normalized arcsinh** (Lupton et al. 2004): `f(x) = asinh(x/β) / asinh(1/β)`, linear near
//!   black (faint detail, low noise gain) and logarithmic in the highlights (compressed cores),
//!   with `β` chosen automatically from the background level.
//! - **Generalized Hyperbolic Stretch (GHS)**: an explicit *designer* curve — a hyperbolic base
//!   mirrored about a symmetry point with linear shadow/highlight protection, spanning the
//!   exponential/logarithmic/hyperbolic family via one parameter `b` (`b ≈ −1.4` ≈ arcsinh).
//!
//! All default to **color-preserving** application: the curve runs on the combined intensity
//! `I = (r+g+b)/3` and every channel is scaled by `f(I)/I`, so hue/saturation and star color are
//! preserved and only intensity is remapped. A per-channel stretch instead ties an object's color
//! to its brightness and burns bright star cores toward white.

use common::Rgb;
use rayon::prelude::*;

use crate::image_ops::{intensity_plane, par_map_pixels};
use crate::math::statistics::{mad_to_sigma, median_and_mad_f32_mut};
use crate::op::{OpError, ensure, require_f32_master};
use imaginarium::Image;

#[cfg(test)]
mod tests;

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
    /// Generalized Hyperbolic Stretch — an explicit *designer* curve (see `ghs.md`). `d` is the
    /// stretch strength (0 = identity); `b` selects the curve family (`0` exponential, `b < 0`
    /// logarithmic-like with `b ≈ −1.4` ≈ asinh, `b > 0` hyperbolic); `sp` is the symmetry point
    /// (most contrast); `lp`/`hp` are the shadow/highlight protection points (linear outside them).
    Ghs {
        d: f32,
        b: f32,
        sp: f32,
        lp: f32,
        hp: f32,
    },
}

/// How a stretch curve is applied across the channels of a color image. No effect on a grayscale
/// image — with one channel, both modes stretch it identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Stretch the combined intensity `I = (r+g+b)/3` and scale each channel by `f(I)/I`.
    /// Preserves hue/saturation and star color; recommended for a final image.
    ColorPreserving,
    /// Stretch each channel independently. For an **auto** method each channel derives its own
    /// parameters from its own statistics, which neutralizes the background toward gray but ties
    /// color to brightness; an explicit method (`Asinh`/`Ghs`) applies the same fixed curve to every
    /// channel (no neutralization). For a quick screen preview.
    PerChannel,
}

/// A stretch to apply to a stacked image. Output is always clamped to `[0, 1]`.
#[derive(Debug, Clone, Copy)]
pub struct Stretch {
    pub method: StretchMethod,
    pub color: ColorMode,
}

impl Stretch {
    /// Color-preserving normalized-arcsinh auto-stretch — the recommended best-quality default.
    pub fn auto_asinh() -> Self {
        Self {
            // A touch gentler than STF's 0.25 — asinh's softer highlights read better slightly darker.
            method: StretchMethod::AutoAsinh {
                target_background: 0.2,
            },
            color: ColorMode::ColorPreserving,
        }
    }

    /// Color-preserving STF (MTF) auto-stretch — the standard automatic "screen stretch".
    pub fn auto_stf() -> Self {
        Self {
            // 0.25 is PixInsight's STF default target background.
            method: StretchMethod::AutoStf {
                shadow_sigmas: 1.5,
                target_background: 0.2,
            },
            color: ColorMode::ColorPreserving,
        }
    }

    /// Color-preserving Generalized Hyperbolic Stretch (see `ghs.md`) with no shadow/highlight
    /// protection (`lp = 0`, `hp = 1`). `d` = strength, `b` = curve family, `sp` = symmetry point.
    pub fn ghs(d: f32, b: f32, sp: f32) -> Self {
        Self {
            method: StretchMethod::Ghs {
                d,
                b,
                sp,
                lp: 0.0,
                hp: 1.0,
            },
            color: ColorMode::ColorPreserving,
        }
    }

    /// Set how the curve is applied across color channels.
    pub fn color(mut self, color: ColorMode) -> Self {
        self.color = color;
        self
    }

    /// Apply this non-linear stretch to a stacked image in place.
    ///
    /// # Errors
    /// [`OpError::UnsupportedFormat`] unless `image` is `L_F32`/`RGB_F32`; [`OpError::InvalidConfig`]
    /// on out-of-range parameters.
    pub fn apply(&self, image: &mut Image) -> Result<(), OpError> {
        self.validate()?;
        require_f32_master(image)?;
        match self.color {
            ColorMode::ColorPreserving => {
                // Auto methods derive the curve from the combined intensity (one curve for the image).
                let curve = explicit_curve(self.method).unwrap_or_else(|| {
                    build_curve(&mut subsample(intensity_plane(image).pixels()), self.method)
                });
                apply_color_preserving_image(image, curve);
            }
            ColorMode::PerChannel => apply_per_channel_image(image, self.method),
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), OpError> {
        match self.method {
            StretchMethod::AutoStf {
                shadow_sigmas,
                target_background,
            } => {
                ensure(shadow_sigmas >= 0.0, || {
                    format!("shadow_sigmas must be >= 0, got {shadow_sigmas}")
                })?;
                ensure_target_background(target_background)
            }
            StretchMethod::AutoAsinh { target_background } => {
                ensure_target_background(target_background)
            }
            StretchMethod::Asinh { beta } => {
                ensure(beta > 0.0, || format!("asinh beta must be > 0, got {beta}"))
            }
            StretchMethod::Ghs { d, b, sp, lp, hp } => {
                ensure(d >= 0.0 && d.is_finite(), || {
                    format!("ghs d must be a finite value >= 0, got {d}")
                })?;
                ensure(b.is_finite(), || format!("ghs b must be finite, got {b}"))?;
                ensure((0.0..=1.0).contains(&sp), || {
                    format!("ghs sp must be in [0, 1], got {sp}")
                })?;
                ensure((0.0..=sp).contains(&lp), || {
                    format!("ghs lp must be in [0, sp], got {lp}")
                })?;
                ensure((sp..=1.0).contains(&hp), || {
                    format!("ghs hp must be in [sp, 1], got {hp}")
                })
            }
        }
    }
}

impl Default for Stretch {
    fn default() -> Self {
        Self::auto_asinh()
    }
}

/// `Ok(())` if `t` is a valid target background in `(0, 1)`.
fn ensure_target_background(t: f32) -> Result<(), OpError> {
    ensure(t > 0.0 && t < 1.0, || {
        format!("target_background must be in (0, 1), got {t}")
    })
}

/// Per-channel stretch on the interleaved image: each channel gets its own auto curve from its own
/// statistics (explicit methods share one curve across channels), applied in place — no deinterleave.
/// Channels are independent, so building channel `c`'s curve from its own samples and writing it back
/// before moving on never reads a value another channel already changed.
fn apply_per_channel_image(image: &mut Image, method: StretchMethod) {
    let nchan = image.desc.color_format.channel_count.channel_count() as usize;
    let samples: &mut [f32] = bytemuck::cast_slice_mut(image.bytes_mut());
    for c in 0..nchan {
        let curve = explicit_curve(method)
            .unwrap_or_else(|| build_curve(&mut subsample_channel(samples, c, nchan), method));
        apply_curve_channel(samples, c, nchan, curve);
    }
}

/// Uniform-stride subsample of channel `channel` from the interleaved `samples` (every `nchan`-th
/// value), capped at `MAX_STRETCH_SAMPLES` for the curve's median/MAD.
fn subsample_channel(samples: &[f32], channel: usize, nchan: usize) -> Vec<f32> {
    let stride = (samples.len() / nchan / MAX_STRETCH_SAMPLES).max(1);
    samples[channel..]
        .iter()
        .step_by(nchan * stride)
        .copied()
        .collect()
}

/// Curves that need no image statistics — built straight from their parameters. Returns `None` for
/// the auto methods, which [`build_curve`] derives from a sample set instead.
fn explicit_curve(method: StretchMethod) -> Option<Curve> {
    match method {
        StretchMethod::Asinh { beta } => Some(Curve::Asinh(AsinhCurve::new(beta))),
        StretchMethod::Ghs { d, b, sp, lp, hp } => {
            Some(Curve::Ghs(GhsCurve::new(d, b, sp, lp, hp)))
        }
        StretchMethod::AutoStf { .. } | StretchMethod::AutoAsinh { .. } => None,
    }
}

/// Cap on the sample count for the auto methods' median/MAD. A robust median+MAD converges far below
/// this, so a uniform-stride subsample is statistically identical to selecting over every pixel —
/// and avoids two full-resolution quickselects (matches `color_calibration` / `denoise`).
const MAX_STRETCH_SAMPLES: usize = 1_000_000;

fn subsample(pixels: &[f32]) -> Vec<f32> {
    let stride = (pixels.len() / MAX_STRETCH_SAMPLES).max(1);
    pixels.iter().step_by(stride).copied().collect()
}

/// A prepared tone curve, selected once from the [`StretchMethod`]. Implementors clamp their
/// output to `[0, 1]`, so any input (including a raw stack's above-unity highlights) yields a
/// valid display value.
trait ToneCurve: Copy + Sync {
    fn eval(&self, x: f32) -> f32;
}

/// STF: a linear clip-rescale `[black, 1] → [0, 1]`, then `MTF(midtones, ·)`.
#[derive(Debug, Clone, Copy)]
struct StfCurve {
    black: f32,
    inv_range: f32,
    midtones: f32,
}

impl StfCurve {
    fn new(median: f32, sigma: f32, shadow_sigmas: f32, target_bkg: f32) -> Self {
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
        Self {
            black,
            inv_range,
            midtones,
        }
    }
}

impl ToneCurve for StfCurve {
    #[inline]
    fn eval(&self, x: f32) -> f32 {
        // The clip-rescale clamps to [0,1] and the MTF maps [0,1] → [0,1], so this is bounded.
        let v = ((x - self.black) * self.inv_range).clamp(0.0, 1.0);
        mtf(self.midtones, v)
    }
}

/// Normalized arcsinh: `asinh(x · inv_beta) · inv_norm`.
#[derive(Debug, Clone, Copy)]
struct AsinhCurve {
    inv_beta: f32,
    inv_norm: f32,
}

impl AsinhCurve {
    fn new(beta: f32) -> Self {
        assert!(
            beta > 0.0,
            "asinh softening beta must be positive, got {beta}"
        );
        let inv_beta = 1.0 / beta;
        Self {
            inv_beta,
            inv_norm: 1.0 / inv_beta.asinh(),
        }
    }
}

impl ToneCurve for AsinhCurve {
    #[inline]
    fn eval(&self, x: f32) -> f32 {
        // asinh maps [0,1] → [0,1] but is unbounded outside it; clamp so a raw stack's above-unity
        // highlights (or negative post-subtraction pixels) still land in display range.
        ((x * self.inv_beta).asinh() * self.inv_norm).clamp(0.0, 1.0)
    }
}

/// `b`-special-cases collapse below this — `b = −1` (logarithmic) and `b = 0` (exponential) are
/// genuine limits where the general forms divide by `b` or `b + 1`.
const GHS_EPS: f32 = 1e-6;

/// GHS base hyperbolic function `T(u)` for `u ≥ 0`, selected on `b` (see `ghs.md`). `T(0) = 0` in
/// every case, which is what makes the curve continuous at the symmetry point.
fn ghs_base_t(d: f32, b: f32, u: f32) -> f32 {
    if (b + 1.0).abs() < GHS_EPS {
        (1.0 + d * u).ln()
    } else if b.abs() < GHS_EPS {
        1.0 - (-d * u).exp()
    } else if b < 0.0 {
        (1.0 - (1.0 - b * d * u).powf((b + 1.0) / b)) / (d * (b + 1.0))
    } else {
        1.0 - (1.0 + b * d * u).powf(-1.0 / b)
    }
}

/// Derivative `T'(u)` of [`ghs_base_t`] — the slope of the linear shadow/highlight tails.
fn ghs_base_tp(d: f32, b: f32, u: f32) -> f32 {
    if (b + 1.0).abs() < GHS_EPS {
        d / (1.0 + d * u)
    } else if b.abs() < GHS_EPS {
        d * (-d * u).exp()
    } else if b < 0.0 {
        (1.0 - b * d * u).powf(1.0 / b)
    } else {
        d * (1.0 + b * d * u).powf(-(1.0 + b) / b)
    }
}

/// Generalized Hyperbolic Stretch: the base curve [`ghs_base_t`] mirrored about `sp`, with linear
/// shadow (`< lp`) and highlight (`> hp`) protection, normalized to map `[0, 1] → [0, 1]`. C¹ and
/// monotonic. Four base evaluations are precomputed here; `eval` does two per pixel. See `ghs.md`.
#[derive(Debug, Clone, Copy)]
struct GhsCurve {
    /// `d ≈ 0` ⇒ the transform is the identity; short-circuit (the normalization would be `0/0`).
    identity: bool,
    d: f32,
    b: f32,
    sp: f32,
    lp: f32,
    hp: f32,
    /// `T(sp − lp)` / `T'(sp − lp)` — the shadow tail's intercept and slope.
    t_sp_lp: f32,
    tp_sp_lp: f32,
    /// `T(hp − sp)` / `T'(hp − sp)` — the highlight tail's intercept and slope.
    t_hp_sp: f32,
    tp_hp_sp: f32,
    /// `t0 = T1(0)` (raw output at 0); `inv_range = 1 / (T4(1) − T1(0))`.
    t0: f32,
    inv_range: f32,
}

impl GhsCurve {
    fn new(d: f32, b: f32, sp: f32, lp: f32, hp: f32) -> Self {
        let zero = Self {
            identity: true,
            d,
            b,
            sp,
            lp,
            hp,
            t_sp_lp: 0.0,
            tp_sp_lp: 0.0,
            t_hp_sp: 0.0,
            tp_hp_sp: 0.0,
            t0: 0.0,
            inv_range: 1.0,
        };
        if d < GHS_EPS {
            return zero;
        }
        let t_sp_lp = ghs_base_t(d, b, sp - lp);
        let tp_sp_lp = ghs_base_tp(d, b, sp - lp);
        let t_hp_sp = ghs_base_t(d, b, hp - sp);
        let tp_hp_sp = ghs_base_tp(d, b, hp - sp);
        let t0 = -lp * tp_sp_lp - t_sp_lp; // T1(0)
        let t1 = (1.0 - hp) * tp_hp_sp + t_hp_sp; // T4(1)
        Self {
            identity: false,
            t_sp_lp,
            tp_sp_lp,
            t_hp_sp,
            tp_hp_sp,
            t0,
            inv_range: 1.0 / (t1 - t0),
            ..zero
        }
    }
}

impl ToneCurve for GhsCurve {
    #[inline]
    fn eval(&self, x: f32) -> f32 {
        if self.identity {
            return x.clamp(0.0, 1.0);
        }
        let raw = if x < self.lp {
            // T1: linear, tangent to the mirrored base at lp.
            self.tp_sp_lp * (x - self.lp) - self.t_sp_lp
        } else if x < self.sp {
            -ghs_base_t(self.d, self.b, self.sp - x) // T2: mirror below sp
        } else if x < self.hp {
            ghs_base_t(self.d, self.b, x - self.sp) // T3: base above sp
        } else {
            // T4: linear, tangent to the base at hp.
            self.tp_hp_sp * (x - self.hp) + self.t_hp_sp
        };
        ((raw - self.t0) * self.inv_range).clamp(0.0, 1.0)
    }
}

/// The curve chosen for a stretch. [`apply_color_preserving_image`] / [`apply_curve_channel`] match
/// this exactly once and then run a monomorphized loop, so the `Stf`/`Asinh` choice is never
/// re-decided per pixel.
#[derive(Debug, Clone, Copy)]
enum Curve {
    Stf(StfCurve),
    Asinh(AsinhCurve),
    Ghs(GhsCurve),
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
    // The reachable target lies in `(median, 1)`. Cap the lower bound at the upper one so a
    // near-white median (not expected on a linear stack, but possible per-channel) can't invert the
    // clamp range and panic.
    let hi = 1.0 - 1e-4;
    let target = target_background.clamp((median + 1e-4).min(hi), hi);
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

/// Build a curve for a statistics-driven (auto) method from a (reorderable) sample set. The explicit
/// methods are resolved by [`explicit_curve`] before any samples are materialized, so they never
/// reach here.
fn build_curve(samples: &mut [f32], method: StretchMethod) -> Curve {
    match method {
        StretchMethod::AutoStf {
            shadow_sigmas,
            target_background,
        } => {
            let (median, mad) = median_and_mad_f32_mut(samples);
            Curve::Stf(StfCurve::new(
                median,
                mad_to_sigma(mad),
                shadow_sigmas,
                target_background,
            ))
        }
        StretchMethod::AutoAsinh { target_background } => {
            let (median, _) = median_and_mad_f32_mut(samples);
            Curve::Asinh(AsinhCurve::new(solve_asinh_beta(median, target_background)))
        }
        StretchMethod::Asinh { .. } | StretchMethod::Ghs { .. } => {
            unreachable!("explicit methods are built by explicit_curve, not build_curve")
        }
    }
}

/// Stretch channel `channel` (every `nchan`-th interleaved sample). Resolves the curve type once,
/// then runs a monomorphized loop.
fn apply_curve_channel(samples: &mut [f32], channel: usize, nchan: usize, curve: Curve) {
    match curve {
        Curve::Stf(c) => map_channel(samples, channel, nchan, c),
        Curve::Asinh(c) => map_channel(samples, channel, nchan, c),
        Curve::Ghs(c) => map_channel(samples, channel, nchan, c),
    }
}

fn map_channel<C: ToneCurve>(samples: &mut [f32], channel: usize, nchan: usize, curve: C) {
    samples
        .par_chunks_mut(nchan)
        .for_each(|px| px[channel] = curve.eval(px[channel]));
}

/// Map one pixel under color-preserving stretch: run `curve` on the combined
/// intensity and scale every channel by `f(I)/I`, with a hue-preserving highlight
/// cap.
fn color_preserve_pixel<C: ToneCurve>(px: Rgb, curve: &C) -> Rgb {
    let intensity = px.intensity();
    // Sub-background pixels (≤ 0, possible after background subtraction) map to black.
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
}

/// Color-preserving stretch on an interleaved `Image`. Resolves the curve type once.
fn apply_color_preserving_image(image: &mut Image, curve: Curve) {
    match curve {
        Curve::Stf(c) => par_map_pixels(image, |l| c.eval(l), |px| color_preserve_pixel(px, &c)),
        Curve::Asinh(c) => par_map_pixels(image, |l| c.eval(l), |px| color_preserve_pixel(px, &c)),
        Curve::Ghs(c) => par_map_pixels(image, |l| c.eval(l), |px| color_preserve_pixel(px, &c)),
    }
}
