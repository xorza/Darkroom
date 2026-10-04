//! Non-linear **stretching**: map a linear, stacked image — faint signal sitting just above a
//! near-black background, stars 10⁴–10⁵× brighter — to a display image with strong contrast
//! across that range. The curve is steep near the background (expanding the thin slice of nebula
//! signal) and shallow in the highlights (compressing star cores so they don't saturate).
//!
//! Input is treated as linear and **already in the pipeline's `[0, 1]` domain** — the decoders
//! establish that, and it is the one place a frame's scale is decided (see
//! [`crate::FitsFloatScale`] for the single input whose header may not settle it). Nothing here
//! measures the frame's own range: a per-frame range would make the stretch depend on content the
//! decoders deliberately refuse to derive a scale from.
//!
//! Excursions outside the domain are still expected and handled — calibration leaves sub-background
//! pixels negative, and a stack's bright stars exceed 1 — so every curve clamps its output, and the
//! result is always a valid display image in `[0, 1]`. A NaN sample shows as black, in the vector
//! paths and the scalar ones alike.
//!
//! Every curve first moves its black point to 0 and keeps white at 1, `(x − black) / (1 − black)`.
//! The automatic methods put it `shadow_sigmas` normalized MADs below the median, PixInsight
//! AutoSTF's black point; an explicit curve takes it as a parameter.
//!
//! Three curve families:
//! - **STF / MTF auto-stretch** (PixInsight/Siril): after the black point, the Midtones Transfer
//!   Function `MTF(m,x) = (m−1)x / ((2m−1)x − m)` — a rational (Möbius) curve, *not* a gamma curve.
//!   The midtones `m` puts the median on the target. Fully automatic; the standard "screen
//!   stretch".
//! - **Normalized arcsinh** (Lupton et al. 2004): `f(x) = asinh(x/β) / asinh(1/β)`, linear near
//!   black (faint detail, low noise gain) and logarithmic in the highlights (compressed cores),
//!   with `β` chosen automatically from the background level.
//! - **Generalized Hyperbolic Stretch (GHS)**: an explicit *designer* curve — a hyperbolic base
//!   mirrored about a symmetry point with linear shadow/highlight protection, spanning the
//!   exponential/logarithmic/hyperbolic family via one parameter `b` (`b ≈ −1.4` ≈ arcsinh).
//!
//! All default to **color-preserving** application: the curve runs on the combined intensity
//! `I = (r+g+b)/3` and every channel is scaled by `f(I)/I`, so hue/saturation and star color are
//! preserved and only intensity is remapped. The ratio is formed after the black point, as Lupton
//! et al. and PixInsight's ArcsinhStretch form it: on data that still holds the sky, a faint
//! nebula's colour is a small excess over a grey pedestal and comes out nearly grey. A per-channel
//! stretch instead ties an object's color to its brightness and burns bright star cores toward
//! white.

use crate::image_ops::rgb::Rgb;
use arrayvec::ArrayVec;
use common::IntrospectEnum;
use rayon::prelude::*;

use crate::error::InvalidConfigField;
use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::image_ops::error::OpError;
use crate::io::image::linear::LinearImage;
use crate::math::statistics::MedianMad;
use crate::math::statistics::subsample::{MAX_STATISTIC_SAMPLES, Subsample};

mod simd;

/// Midtones balance is clamped away from the degenerate endpoints `0`/`1`, where the MTF
/// collapses every interior value onto a single output.
const MIDTONES_MIN: f32 = 1e-4;
const MIDTONES_MAX: f32 = 1.0 - 1e-4;

/// Which stretch curve to apply, and how its parameters are chosen.
#[derive(Debug, Clone, Copy)]
pub enum StretchMethod {
    /// Screen-Transfer-Function (MTF) auto-stretch. Black point `= median − shadow_sigmas·σ`
    /// (σ the normalized MAD), and the midtones balance is chosen so the rescaled median lands on
    /// `target_background`.
    AutoStf {
        shadow_sigmas: f32,
        target_background: f32,
    },
    /// Normalized arcsinh after the same black point as [`Self::AutoStf`], with softening `β`
    /// chosen so the rescaled median maps to `target_background`.
    AutoAsinh {
        shadow_sigmas: f32,
        target_background: f32,
    },
    /// Normalized arcsinh after `black_point`, with an explicit softening `β` (smaller = stronger
    /// stretch).
    Asinh { black_point: f32, beta: f32 },
    /// Generalized Hyperbolic Stretch after `black_point` — an explicit *designer* curve. `d` is
    /// the stretch strength (0 = identity); `b` selects the curve family (`0` exponential, `b < 0`
    /// logarithmic-like with `b ≈ −1.4` ≈ asinh, `b > 0` hyperbolic); `sp` is the symmetry point
    /// (most contrast); `lp`/`hp` are the shadow/highlight protection points (linear outside them).
    Ghs {
        black_point: f32,
        d: f32,
        b: f32,
        sp: f32,
        lp: f32,
        hp: f32,
    },
}

/// How a stretch curve is applied across the channels of a color image. No effect on a grayscale
/// image — with one channel, both modes stretch it identically.
///
/// `type_id` is this enum's identity to an introspecting consumer; that
/// consumer stores it, so it is fixed for the life of the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "ca1be21f-2096-410c-8bc2-33e96d9b12be")]
pub enum ColorMode {
    /// Stretch the combined intensity `I = (r+g+b)/3` and scale each channel by `f(I)/I`.
    /// Preserves hue/saturation and star color; recommended for a final image.
    ColorPreserving,
    /// Stretch each channel independently. For an **auto** method each channel derives its own
    /// parameters from its own statistics, which neutralizes the background toward gray but ties
    /// color to brightness; an explicit method (`Asinh`/`Ghs`) applies the same fixed curve to
    /// every channel (no neutralization). For a quick screen preview.
    PerChannel,
}

impl StretchMethod {
    /// The background level the automatic presets place the median on: PixInsight AutoSTF's.
    pub const AUTO_TARGET_BACKGROUND: f32 = 0.25;
    /// How many normalized MADs below the median the automatic presets put the black point:
    /// PixInsight AutoSTF's shadows clipping, −2.8.
    pub const AUTO_SHADOW_SIGMAS: f32 = 2.8;
}

/// A stretch to apply to a stacked image. Output is always clamped to `[0, 1]`.
#[derive(Debug, Clone, Copy)]
pub struct Stretch {
    pub method: StretchMethod,
    pub color: ColorMode,
}

impl Stretch {
    /// Color-preserving normalized-arcsinh auto-stretch — the recommended best-quality default.
    pub const fn auto_asinh() -> Self {
        Self {
            method: StretchMethod::AutoAsinh {
                shadow_sigmas: StretchMethod::AUTO_SHADOW_SIGMAS,
                target_background: StretchMethod::AUTO_TARGET_BACKGROUND,
            },
            color: ColorMode::ColorPreserving,
        }
    }

    /// Color-preserving STF (MTF) auto-stretch — the standard automatic "screen stretch".
    pub const fn auto_stf() -> Self {
        Self {
            method: StretchMethod::AutoStf {
                shadow_sigmas: StretchMethod::AUTO_SHADOW_SIGMAS,
                target_background: StretchMethod::AUTO_TARGET_BACKGROUND,
            },
            color: ColorMode::ColorPreserving,
        }
    }

    /// Apply this non-linear stretch to a stacked image in place.
    ///
    /// # Errors
    /// [`OpError::InvalidConfig`] on out-of-range parameters, and
    /// [`OpError::UnreachableBackground`] when an auto method cannot place the measured background
    /// on its target. The image is unchanged on error.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        match self.color {
            ColorMode::ColorPreserving => {
                // Auto methods derive the curve from the combined intensity (one curve for the
                // image).
                let curve = match explicit_curve(self.method) {
                    Some(curve) => curve,
                    None => build_curve(&mut subsample_intensity(image), self.method)?,
                };
                apply_color_preserving_image(image, curve);
            }
            ColorMode::PerChannel => apply_per_channel_image(image, self.method)?,
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), InvalidConfigField> {
        match self.method {
            StretchMethod::AutoStf {
                shadow_sigmas,
                target_background,
            }
            | StretchMethod::AutoAsinh {
                shadow_sigmas,
                target_background,
            } => {
                InvalidConfigField::finite(
                    "shadow_sigmas",
                    "finite and non-negative",
                    shadow_sigmas,
                    |value| value >= 0.0,
                )?;
                ensure_target_background(target_background)
            }
            StretchMethod::Asinh { black_point, beta } => {
                ensure_black_point(black_point)?;
                InvalidConfigField::finite("asinh beta", "finite and positive", beta, |value| {
                    value > 0.0
                })
            }
            StretchMethod::Ghs {
                black_point,
                d,
                b,
                sp,
                lp,
                hp,
            } => {
                ensure_black_point(black_point)?;
                InvalidConfigField::finite("ghs d", "finite and non-negative", d, |value| {
                    value >= 0.0
                })?;
                InvalidConfigField::finite_only("ghs b", b)?;
                InvalidConfigField::finite("ghs sp", "finite and in [0, 1]", sp, |value| {
                    (0.0..=1.0).contains(&value)
                })?;
                InvalidConfigField::check_against(
                    (0.0..=sp).contains(&lp),
                    "ghs lp",
                    "in [0, sp]",
                    lp,
                    sp,
                )?;
                InvalidConfigField::check_against(
                    (sp..=1.0).contains(&hp),
                    "ghs hp",
                    "in [sp, 1]",
                    hp,
                    sp,
                )
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
fn ensure_target_background(t: f32) -> Result<(), InvalidConfigField> {
    InvalidConfigField::finite("target_background", "finite and in (0, 1)", t, |value| {
        value > 0.0 && value < 1.0
    })
}

/// `Ok(())` if `black` is a valid black point in `[0, 1)`: one at white leaves no range to map.
fn ensure_black_point(black: f32) -> Result<(), InvalidConfigField> {
    InvalidConfigField::finite("black_point", "finite and in [0, 1)", black, |value| {
        (0.0..1.0).contains(&value)
    })
}

/// Per-channel stretch: each channel gets its own auto curve from its own statistics (explicit
/// methods share one curve across channels), applied to its own plane. Channels are independent, so
/// nothing here reads a value another channel already changed.
///
/// Every channel's curve is built before any channel is written, so a channel whose background is
/// out of reach leaves the whole image untouched.
fn apply_per_channel_image(image: &mut LinearImage, method: StretchMethod) -> Result<(), OpError> {
    let mut curves = ArrayVec::<Curve, 3>::new();
    for plane in image.planes_mut() {
        curves.push(match explicit_curve(method) {
            Some(curve) => curve,
            None => build_curve(&mut Subsample::statistic_values(plane.pixels()), method)?,
        });
    }
    for (plane, curve) in image.planes_mut().zip(curves) {
        apply_curve_plane(plane.pixels_mut(), curve);
    }
    Ok(())
}

/// Curves that need no image statistics — built straight from their parameters. Returns `None` for
/// the auto methods, which [`build_curve`] derives from a sample set instead.
fn explicit_curve(method: StretchMethod) -> Option<Curve> {
    match method {
        StretchMethod::Asinh { black_point, beta } => Some(Curve {
            black: BlackPoint::new(black_point),
            tone: Tone::Asinh(AsinhCurve::new(beta)),
        }),
        StretchMethod::Ghs {
            black_point,
            d,
            b,
            sp,
            lp,
            hp,
        } => Some(Curve {
            black: BlackPoint::new(black_point),
            tone: Tone::Ghs(GhsCurve::new(d, b, sp, lp, hp)),
        }),
        StretchMethod::AutoStf { .. } | StretchMethod::AutoAsinh { .. } => None,
    }
}

/// Uniform-stride subsample of the combined intensity `I = (r+g+b)/3` (the sample itself for mono),
/// computed from the planes directly — never materializing the full intensity plane just to throw
/// all but every `stride`-th value away. Identical samples to subsampling
/// [`LinearImage::intensity_plane`](crate::io::image::linear::LinearImage::intensity_plane).
fn subsample_intensity(image: &LinearImage) -> Vec<f32> {
    let plane = image.channel(0).pixels();
    if !image.is_rgb() {
        return Subsample::statistic_values(plane);
    }
    let sample = Subsample::new(plane.len(), MAX_STATISTIC_SAMPLES);
    let (g, b) = (image.channel(1).pixels(), image.channel(2).pixels());
    // A stride-`n` walk over three planes costs three cache lines and three TLB streams per sampled
    // pixel; running it in parallel hides that latency — the work is a pure map over indices.
    (0..sample.count())
        .into_par_iter()
        .map(|k| {
            let i = sample.index(k);
            Rgb {
                r: plane[i],
                g: g[i],
                b: b[i],
            }
            .intensity()
        })
        .collect()
}

/// The black point a curve moves to 0 first, keeping white at 1: `(x − black) / (1 − black)`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BlackPoint {
    black: f32,
    inv_range: f32,
}

impl BlackPoint {
    /// `black` in `[0, 1)`; 0 maps every value to itself.
    fn new(black: f32) -> Self {
        debug_assert!((0.0..1.0).contains(&black), "black point {black}");
        Self {
            black,
            inv_range: 1.0 / (1.0 - black),
        }
    }

    #[inline]
    fn rescale(self, x: f32) -> f32 {
        (x - self.black) * self.inv_range
    }
}

/// An automatic method's black point, and the background median after it.
#[derive(Debug, Clone, Copy)]
struct AutoBackground {
    black: BlackPoint,
    rescaled: f32,
}

impl AutoBackground {
    /// The black point `shadow_sigmas` times `sigma` below `median`, held at 0 from below;
    /// `unreachable` when it reaches white or leaves the median at or below it, or at white —
    /// nothing for a curve to place.
    fn new(
        median: f32,
        sigma: f32,
        shadow_sigmas: f32,
        unreachable: impl FnOnce() -> OpError,
    ) -> Result<Self, OpError> {
        let black = (median - shadow_sigmas * sigma).max(0.0);
        if black >= 1.0 {
            return Err(unreachable());
        }
        let black = BlackPoint::new(black);
        let rescaled = black.rescale(median);
        if !(rescaled > 0.0 && rescaled < 1.0) {
            return Err(unreachable());
        }
        Ok(Self { black, rescaled })
    }
}

/// A prepared tone curve, selected once from the [`StretchMethod`], on values after the black
/// point. Implementors clamp their output to `[0, 1]`, so any input (including a raw stack's
/// above-unity highlights) yields a valid display value, and map NaN to 0.
trait ToneCurve: Copy + Sync {
    fn eval(&self, x: f32) -> f32;

    /// [`Self::eval`] after `black` over a block of samples, in place — where a curve with a vector
    /// kernel takes it over.
    fn eval_block(&self, black: BlackPoint, block: &mut [f32]) {
        for value in block {
            *value = self.eval(black.rescale(*value));
        }
    }
}

/// `x` held to `[0, 1]`, NaN to 0: `f32::max` takes the operand that is not NaN.
#[inline]
const fn unit(x: f32) -> f32 {
    x.max(0.0).min(1.0)
}

/// `MTF(midtones, ·)`, the STF's curve after its black point.
#[derive(Debug, Clone, Copy)]
struct MtfCurve {
    midtones: f32,
}

impl MtfCurve {
    /// The curve that puts the rescaled `median` on `target`; `None` when no midtones balance in
    /// `[MIDTONES_MIN, MIDTONES_MAX]` does.
    fn new(median: f32, target: f32) -> Option<Self> {
        // MTF's Möbius self-inverse identity: MTF(MTF(t, x0), x0) = t, so the midtones balance
        // that maps the rescaled median onto the target background is just MTF(target, median).
        let midtones = mtf(target, median);
        (MIDTONES_MIN..=MIDTONES_MAX)
            .contains(&midtones)
            .then_some(Self { midtones })
    }
}

impl ToneCurve for MtfCurve {
    #[inline]
    fn eval(&self, x: f32) -> f32 {
        mtf(self.midtones, unit(x))
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
        unit((x * self.inv_beta).asinh() * self.inv_norm)
    }

    fn eval_block(&self, black: BlackPoint, block: &mut [f32]) {
        simd::asinh_plane(block, black, *self);
    }
}

/// GHS base hyperbolic function `T(u)` for `u ≥ 0`, selected on `b`. `T(0) = 0` in every case,
/// which is what makes the curve continuous at the symmetry point.
///
/// Written through `ln_1p`/`expm1` so it stays accurate as `b` nears its two limits: the textbook
/// `1 − (1 + b·d·u)^(−1/b)` cancels as `b → 0` (in f32, 18.6% off at `b = 2e-6`, `d = 5`) and the
/// `b < 0` form divides by `b + 1` as `b → −1`. Only the limits themselves, `b = 0` (exponential)
/// and `b = −1` (logarithmic), take their own forms — the general ones are `0/0` there exactly.
/// Each branch fixes its own scale, which the curve's normalization divides out.
fn ghs_base_t(d: f32, b: f32, u: f32) -> f32 {
    if b == 0.0 {
        -(-d * u).exp_m1()
    } else if b == -1.0 {
        (d * u).ln_1p() / d
    } else if b < 0.0 {
        -(((b + 1.0) / b) * (-b * d * u).ln_1p()).exp_m1() / (d * (b + 1.0))
    } else {
        -(-(b * d * u).ln_1p() / b).exp_m1()
    }
}

/// Derivative `T'(u)` of [`ghs_base_t`] — the slope of the linear shadow/highlight tails.
fn ghs_base_tp(d: f32, b: f32, u: f32) -> f32 {
    if b == 0.0 {
        d * (-d * u).exp()
    } else if b < 0.0 {
        ((-b * d * u).ln_1p() / b).exp()
    } else {
        d * (-((1.0 + b) / b) * (b * d * u).ln_1p()).exp()
    }
}

/// Generalized Hyperbolic Stretch: the base curve [`ghs_base_t`] mirrored about `sp`, with linear
/// shadow (`< lp`) and highlight (`> hp`) protection, normalized to map `[0, 1] → [0, 1]`. C¹ and
/// monotonic. Four base evaluations are precomputed here; `eval` does two per pixel.
#[derive(Debug, Clone, Copy)]
struct GhsCurve {
    /// `d = 0`, or a range too small to normalize: the transform is the identity.
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
        let t_sp_lp = ghs_base_t(d, b, sp - lp);
        let tp_sp_lp = ghs_base_tp(d, b, sp - lp);
        let t_hp_sp = ghs_base_t(d, b, hp - sp);
        let tp_hp_sp = ghs_base_tp(d, b, hp - sp);
        let t0 = -lp * tp_sp_lp - t_sp_lp; // T1(0)
        let t1 = (1.0 - hp) * tp_hp_sp + t_hp_sp; // T4(1)
        // `d = 0` is the identity, and a `d` so small that the curve's range underflows is it too:
        // the normalization would divide by zero.
        if d == 0.0 || !(t1 - t0).is_normal() {
            return zero;
        }
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
            return unit(x);
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
        unit((raw - self.t0) * self.inv_range)
    }
}

/// The curve chosen for a stretch: its black point, then its tone curve.
/// [`apply_color_preserving_image`] / [`apply_curve_plane`] match the tone exactly once and then
/// run a monomorphized loop, so the choice is never re-decided per pixel.
#[derive(Debug, Clone, Copy)]
struct Curve {
    black: BlackPoint,
    tone: Tone,
}

#[derive(Debug, Clone, Copy)]
enum Tone {
    Mtf(MtfCurve),
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

/// Choose the arcsinh softening `β` so a background of `median` maps to `target_background`;
/// `None` when no `β` does.
///
/// `g(β) = asinh(median/β) / asinh(1/β)` is monotonically decreasing in `β`, ranging from ~1 as
/// `β → 0` (strong, log-like) to `median` as `β → ∞` (near-linear), so a reachable target lies in
/// `(median, 1)`. Bisect `log₁₀ β` over `[−30, 5]` — wide enough for medians down to ~1e-20 — and
/// then check the result, because a target past the range's end converges onto the bound rather
/// than onto the target.
fn solve_asinh_beta(median: f32, target_background: f32) -> Option<f32> {
    if !(median > 0.0 && median < target_background) {
        return None;
    }
    let g = |beta: f32| (median / beta).asinh() / (1.0 / beta).asinh();
    let (mut lo, mut hi) = (-30.0f32, 5.0f32);
    for _ in 0..60 {
        let mid = f32::midpoint(lo, hi);
        if g(10.0f32.powf(mid)) > target_background {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let beta = 10.0f32.powf(f32::midpoint(lo, hi));
    // A reachable target is met to the f32 accuracy of the two `asinh` evaluations (a few ulp,
    // ~1e-6); a target past the range's end misses by far more than this.
    ((g(beta) - target_background).abs() <= 1e-4).then_some(beta)
}

/// Build a curve for a statistics-driven (auto) method from a (reorderable) sample set. The
/// explicit methods are resolved by [`explicit_curve`] before any samples are materialized, so they
/// never reach here.
fn build_curve(samples: &mut [f32], method: StretchMethod) -> Result<Curve, OpError> {
    let background = MedianMad::of_mut(samples);
    match method {
        StretchMethod::AutoStf {
            shadow_sigmas,
            target_background,
        } => stf_curve(
            background.median,
            background.sigma(),
            shadow_sigmas,
            target_background,
        ),
        StretchMethod::AutoAsinh {
            shadow_sigmas,
            target_background,
        } => auto_asinh_curve(
            background.median,
            background.sigma(),
            shadow_sigmas,
            target_background,
        ),
        StretchMethod::Asinh { .. } | StretchMethod::Ghs { .. } => {
            unreachable!("explicit methods are built by explicit_curve, not build_curve")
        }
    }
}

/// The STF curve for a background of `median` and spread `sigma`: the black point
/// `shadow_sigmas·sigma` below the median, and the midtones that put the median on `target`.
fn stf_curve(median: f32, sigma: f32, shadow_sigmas: f32, target: f32) -> Result<Curve, OpError> {
    let unreachable = || OpError::UnreachableBackground {
        method: "auto STF",
        median,
        target,
    };
    let background = AutoBackground::new(median, sigma, shadow_sigmas, unreachable)?;
    let tone = MtfCurve::new(background.rescaled, target).ok_or_else(unreachable)?;
    Ok(Curve {
        black: background.black,
        tone: Tone::Mtf(tone),
    })
}

/// The auto-asinh curve for a background of `median` and spread `sigma`: the black point
/// [`stf_curve`] takes, and the softening that puts the median on `target`.
fn auto_asinh_curve(
    median: f32,
    sigma: f32,
    shadow_sigmas: f32,
    target: f32,
) -> Result<Curve, OpError> {
    let unreachable = || OpError::UnreachableBackground {
        method: "auto asinh",
        median,
        target,
    };
    let background = AutoBackground::new(median, sigma, shadow_sigmas, unreachable)?;
    let beta = solve_asinh_beta(background.rescaled, target).ok_or_else(unreachable)?;
    Ok(Curve {
        black: background.black,
        tone: Tone::Asinh(AsinhCurve::new(beta)),
    })
}

/// Stretch one plane. Resolves the curve type once, then runs a monomorphized loop.
fn apply_curve_plane(plane: &mut [f32], curve: Curve) {
    match curve.tone {
        Tone::Mtf(c) => map_plane(plane, curve.black, c),
        Tone::Asinh(c) => map_plane(plane, curve.black, c),
        Tone::Ghs(c) => map_plane(plane, curve.black, c),
    }
}

fn map_plane<C: ToneCurve>(plane: &mut [f32], black: BlackPoint, curve: C) {
    plane
        .par_chunks_mut(SAMPLES_PER_BLOCK)
        .for_each(|block| curve.eval_block(black, block));
}

/// Map one pixel under color-preserving stretch: every channel after the black point, `curve` on
/// their combined intensity, and the pixel moved to that intensity with its hue kept
/// ([`Rgb::with_intensity`]).
fn color_preserve_pixel<C: ToneCurve>(px: Rgb, black: BlackPoint, curve: &C) -> Rgb {
    let px = Rgb {
        r: black.rescale(px.r),
        g: black.rescale(px.g),
        b: black.rescale(px.b),
    };
    px.with_intensity(curve.eval(px.intensity()))
}

/// Color-preserving stretch. Resolves the curve type once.
///
/// On a grayscale image the combined intensity *is* the single channel, so "scale each channel by
/// `f(I)/I`" reduces to the curve on that plane.
fn apply_color_preserving_image(image: &mut LinearImage, curve: Curve) {
    if !image.is_rgb() {
        for plane in image.planes_mut() {
            apply_curve_plane(plane.pixels_mut(), curve);
        }
        return;
    }
    let black = curve.black;
    match curve.tone {
        Tone::Mtf(c) => image.map_rgb(|px| color_preserve_pixel(px, black, &c)),
        Tone::Asinh(c) => apply_color_preserving_asinh(image, black, c),
        Tone::Ghs(c) => image.map_rgb(|px| color_preserve_pixel(px, black, &c)),
    }
}

/// Color-preserving arcsinh on an **RGB** image, band-parallel across the three planes. The curve
/// itself, and the kernel that evaluates it, live in [`simd`].
fn apply_color_preserving_asinh(image: &mut LinearImage, black: BlackPoint, c: AsinhCurve) {
    debug_assert!(image.is_rgb(), "caller dispatches grayscale to map_samples");
    let [r, g, b] = image.rgb_planes_mut();
    // The three planes split in lockstep so each task sees one band's worth of every channel.
    r.par_chunks_mut(SAMPLES_PER_BLOCK)
        .zip(g.par_chunks_mut(SAMPLES_PER_BLOCK))
        .zip(b.par_chunks_mut(SAMPLES_PER_BLOCK))
        .for_each(|((r, g), b)| simd::asinh_color_preserve(r, g, b, black, c));
}

#[cfg(test)]
mod internals {
    use crate::image_ops::stretching::{Curve, Tone, ToneCurve};

    impl Curve {
        /// The curve at `x`, its black point first.
        pub(super) fn eval(&self, x: f32) -> f32 {
            let x = self.black.rescale(x);
            match self.tone {
                Tone::Mtf(c) => c.eval(x),
                Tone::Asinh(c) => c.eval(x),
                Tone::Ghs(c) => c.eval(x),
            }
        }
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
