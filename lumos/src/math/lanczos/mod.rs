//! The Lanczos windowed-sinc kernel, shared by everything that resamples.

use std::f64::consts::PI;

/// `sinc(πx) · sinc(πx/a)` — the Lanczos kernel of half-width `a`, zero beyond it — in f64,
/// rounded once to f32. Each `sin` reads its argument reduced exactly (see [`sin_pi`]), so the
/// kernel's zeros at the integers are exact zeros; an f32 `sin(π·x)` reads the rounded product,
/// which near those zeros, where the slope is largest, is off by up to 1e-6.
///
/// One definition for both resamplers: `registration::resample` builds its lookup table from this,
/// and `drizzle` evaluates it per drop for the Lanczos kernel. They are peer subsystems, so it
/// lives here rather than in either of them.
#[inline]
pub(crate) fn kernel(x: f32, a: f32) -> f32 {
    let x = f64::from(x.abs());
    let a = f64::from(a);
    if x == 0.0 {
        return 1.0;
    }
    if x >= a {
        return 0.0;
    }
    let x_a = x / a;
    ((sin_pi(x) / (PI * x)) * (sin_pi(x_a) / (PI * x_a))) as f32
}

/// `sin(πt)` from `t` less its nearest integer, which is exact in f64: the zeros at the integers
/// come out exact, and the one rounded product `π·r` is of an `|r| ≤ ½`.
fn sin_pi(t: f64) -> f64 {
    let whole = t.round();
    let sine = (PI * (t - whole)).sin();
    if whole % 2.0 == 0.0 { sine } else { -sine }
}

#[cfg(test)]
mod tests;
