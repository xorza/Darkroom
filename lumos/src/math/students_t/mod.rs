//! Student's t distribution at a whole number of degrees of freedom: its upper quantile, over
//! `common::Libm`, so it has the same bits on every system.

use std::f64::consts::{FRAC_1_PI, LN_2, PI, SQRT_2};
use std::num::NonZeroU32;

use common::Libm;

use crate::math::normal;

/// The most Newton steps [`upper_quantile`] takes; each roughly doubles the digits from the
/// expansion's start, which holds two or more.
const NEWTON_STEPS_MAX: usize = 32;

/// The most continued-fraction terms [`Point::ln_two_sided_tail`] takes. On the side of the
/// switch where it runs, the fraction converges in `O(√ν)` terms (*Numerical Recipes* §6.4).
const FRACTION_TERMS_MAX: usize = 100_000;

/// The `t` with `P(T > t) = tail`, for `0 < tail < ½` at `df` degrees of freedom.
///
/// One and two degrees take their closed forms, `cot(π·tail)` and
/// `(1 − 2·tail)/√(2·tail·(1 − tail))`. More take the expansion about the normal quantile
/// (Abramowitz & Stegun 26.7.5) and then Newton's steps on `ln P(|T| > t)` in `ln t`, to 4ε of
/// `ln t` or until a step stops shrinking, as R's `qt` refines Hill's (1970) start. The tail is
/// the upper one, not `1 − p`, so a small one keeps its digits.
///
/// # Panics
///
/// Unless `tail` is in `(0, ½)`.
pub(crate) fn upper_quantile(df: NonZeroU32, tail: f64) -> f64 {
    assert!(tail > 0.0, "an upper t quantile of tail {tail}");
    assert!(tail < 0.5, "an upper t quantile of tail {tail}");
    match df.get() {
        1 => 1.0 / (PI * tail).libm_tan(),
        2 => (1.0 - 2.0 * tail) / (2.0 * tail * (1.0 - tail)).sqrt(),
        _ => newton(df.get(), tail),
    }
}

/// [`upper_quantile`] past two degrees: the expansion's start, then Newton's steps on
/// `ln P(|T| > t)` against `ln t`, which the tail's power law `P ~ C·t^(−ν)` makes nearly a line,
/// so a start far out converges in a few steps. Every quantity is a log, so a tail far past the
/// density's underflow still places its quantile.
fn newton(df: u32, tail: f64) -> f64 {
    let nu = f64::from(df);
    let target = (2.0 * tail).libm_ln();
    let ln_scale = density_scale(df).libm_ln();
    let mut ln_t = expansion(nu, -normal::quantile(tail)).libm_ln();
    let mut previous = f64::INFINITY;
    for _ in 0..NEWTON_STEPS_MAX {
        let point = Point::new(nu, ln_t);
        let ln_both = point.ln_two_sided_tail(ln_scale);
        let ln_density = ln_scale + f64::midpoint(nu, 1.0) * point.ln_x;
        // `ln P(|T| > t)` falls with slope `−2·t·f(t)/P` in `ln t`, so the step is the gap over it.
        let step = (ln_both - target) * (ln_both - LN_2 - ln_t - ln_density).libm_exp();
        // A step that no longer shrinks is the tail's own rounding: the quantile is as close as
        // `f64` places it.
        if step.abs() >= previous.abs() {
            return ln_t.libm_exp();
        }
        ln_t += step;
        if step.abs() <= 4.0 * f64::EPSILON * ln_t.abs().max(1.0) {
            return ln_t.libm_exp();
        }
        previous = step;
    }
    unreachable!("Newton's steps from the expansion converge on a t quantile at {df} df, {tail}");
}

/// A `t` at `ν` degrees, by the logs of `x = ν/(ν + t²) = 1/(1 + r²)` and
/// `1 − x = 1/(1 + 1/r²)`, `r = t/√ν`, which the incomplete beta reads: each a `ln_1p` with no
/// cancellation, so `a·ln x` keeps its digits at many degrees, and its square taken only where it
/// cannot overflow.
#[derive(Debug, Clone, Copy)]
struct Point {
    nu: f64,
    ln_x: f64,
    ln_one_minus_x: f64,
}

impl Point {
    /// Past `r = 10¹⁵⁰` its square, and below `10⁻¹⁵⁰` its inverse's, would overflow; there
    /// `ln(1 + r²)` is `2·ln r` to far within ε.
    const SQUARE_SAFE: f64 = 1e150;

    fn new(nu: f64, ln_t: f64) -> Self {
        let ln_r = ln_t - 0.5 * nu.libm_ln();
        let r = ln_r.libm_exp();
        let ln_x = if r > Self::SQUARE_SAFE {
            -2.0 * ln_r
        } else {
            -(r * r).libm_ln_1p()
        };
        let ln_one_minus_x = if r < 1.0 / Self::SQUARE_SAFE {
            2.0 * ln_r
        } else {
            -(1.0 / (r * r)).libm_ln_1p()
        };
        Self {
            nu,
            ln_x,
            ln_one_minus_x,
        }
    }

    /// `ln P(|T| > t)`, the regularized incomplete beta `I_x(ν/2, ½)`. Its continued fraction runs
    /// where it converges fast, `x < (a + 1)/(a + b + 2)`, which is the tail itself, so a small
    /// tail keeps its digits; elsewhere the complement `1 − I_{1−x}(½, ν/2)`, where the tail is
    /// large (*Numerical Recipes* §6.4, `betai`). `ln_scale` is the log of [`density_scale`].
    fn ln_two_sided_tail(self, ln_scale: f64) -> f64 {
        let (a, b) = (self.nu / 2.0, 0.5);
        // `ln(x^a·(1 − x)^b / B(a, b))`, with `1/B(ν/2, ½) = √ν·` the density's constant.
        let ln_front = ln_scale + 0.5 * self.nu.libm_ln() + a * self.ln_x + b * self.ln_one_minus_x;
        let x = self.ln_x.libm_exp();
        if x < (a + 1.0) / (a + b + 2.0) {
            ln_front + (continued_fraction(a, b, x) / a).libm_ln()
        } else {
            let one_minus_x = self.ln_one_minus_x.libm_exp();
            (-(ln_front.libm_exp() * continued_fraction(b, a, one_minus_x) / b)).libm_ln_1p()
        }
    }
}

/// The Cornish–Fisher expansion of the t quantile about the normal one `z`, to `ν⁻⁴`
/// (Abramowitz & Stegun 26.7.5).
fn expansion(nu: f64, z: f64) -> f64 {
    let z2 = z * z;
    let g1 = (z2 + 1.0) * z / 4.0;
    let g2 = ((5.0 * z2 + 16.0) * z2 + 3.0) * z / 96.0;
    let g3 = (((3.0 * z2 + 19.0) * z2 + 17.0) * z2 - 15.0) * z / 384.0;
    let g4 = ((((79.0 * z2 + 776.0) * z2 + 1482.0) * z2 - 1920.0) * z2 - 945.0) * z / 92160.0;
    z + (g1 + (g2 + (g3 + g4 / nu) / nu) / nu) / nu
}

/// The density's constant `Γ((ν + 1)/2) / (√(νπ)·Γ(ν/2))`, by its recurrence from `1/π` at one
/// degree and `1/(2√2)` at two: each step of two degrees multiplies it by `(ν + 1)/ν·√(ν/(ν + 2))`.
fn density_scale(df: u32) -> f64 {
    let (mut scale, mut nu) = if df % 2 == 1 {
        (FRAC_1_PI, 1)
    } else {
        (1.0 / (2.0 * SQRT_2), 2)
    };
    while nu < df {
        let at = f64::from(nu);
        scale *= (at + 1.0) / at * (at / (at + 2.0)).sqrt();
        nu += 2;
    }
    scale
}

/// The continued fraction of the incomplete beta `I_x(a, b)`, by Lentz's method (*Numerical
/// Recipes* §6.4, `betacf`), to ε.
fn continued_fraction(a: f64, b: f64, x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    let guard = |value: f64| if value.abs() < TINY { TINY } else { value };
    let (sum, plus, minus) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 / guard(1.0 - sum * x / plus);
    let mut h = d;
    for m in 1..=FRACTION_TERMS_MAX {
        let m = m as f64;
        let m2 = 2.0 * m;
        let even = m * (b - m) * x / ((minus + m2) * (a + m2));
        d = 1.0 / guard(1.0 + even * d);
        c = guard(1.0 + even / c);
        h *= d * c;
        let odd = -(a + m) * (sum + m) * x / ((a + m2) * (plus + m2));
        d = 1.0 / guard(1.0 + odd * d);
        c = guard(1.0 + odd / c);
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() <= f64::EPSILON {
            return h;
        }
    }
    unreachable!("the incomplete beta's fraction converges at a {a}, b {b}, x {x}");
}

#[cfg(test)]
mod tests;
