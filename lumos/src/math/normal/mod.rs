//! The standard normal distribution: its density, its cdf and its quantile, over `common::Libm`,
//! so each has the same bits on every system.

use std::f64::consts::{FRAC_1_SQRT_2, PI, SQRT_2};

use common::Libm;

/// AS 241's central rational form, numerator then denominator after its leading 1, for
/// `|p − ½| ≤ 0.425`; Wichura's coefficients as R's `qnorm` holds them.
#[expect(
    clippy::excessive_precision,
    reason = "the coefficients as Wichura publishes them"
)]
const CENTRAL_P: [f64; 8] = [
    2509.0809287301226727,
    33430.575583588128105,
    67265.770927008700853,
    45921.953931549871457,
    13731.693765509461125,
    1971.5909503065514427,
    133.14166789178437745,
    3.387132872796366608,
];
#[expect(
    clippy::excessive_precision,
    reason = "the coefficients as Wichura publishes them"
)]
const CENTRAL_Q: [f64; 7] = [
    5226.495278852854561,
    28729.085735721942674,
    39307.89580009271061,
    21213.794301586595867,
    5394.1960214247511077,
    687.1870074920579083,
    42.313330701600911252,
];

/// AS 241's form for the tails where `r = √(−ln min(p, 1 − p)) ≤ 5`, in `r − 1.6`.
#[expect(
    clippy::excessive_precision,
    reason = "the coefficients as Wichura publishes them"
)]
const NEAR_P: [f64; 8] = [
    7.7454501427834140764e-4,
    0.0227238449892691845833,
    0.24178072517745061177,
    1.27045825245236838258,
    3.64784832476320460504,
    5.7694972214606914055,
    4.6303378461565452959,
    1.42343711074968357734,
];
#[expect(
    clippy::excessive_precision,
    reason = "the coefficients as Wichura publishes them"
)]
const NEAR_Q: [f64; 7] = [
    1.05075007164441684324e-9,
    5.475938084995344946e-4,
    0.0151986665636164571966,
    0.14810397642748007459,
    0.68976733498510000455,
    1.6763848301838038494,
    2.05319162663775882187,
];

/// AS 241's form for the far tails, `r > 5`, in `r − 5`.
#[expect(
    clippy::excessive_precision,
    reason = "the coefficients as Wichura publishes them"
)]
const FAR_P: [f64; 8] = [
    2.01033439929228813265e-7,
    2.71155556874348757815e-5,
    0.0012426609473880784386,
    0.026532189526576123093,
    0.29656057182850489123,
    1.7848265399172913358,
    5.4637849111641143699,
    6.6579046435011037772,
];
#[expect(
    clippy::excessive_precision,
    reason = "the coefficients as Wichura publishes them"
)]
const FAR_Q: [f64; 7] = [
    2.04426310338993978564e-15,
    1.4215117583164458887e-7,
    1.8463183175100546818e-5,
    7.868691311456132591e-4,
    0.0148753612908506148525,
    0.13692988092273580531,
    0.59983220655588793769,
];

/// `1/√2 − FRAC_1_SQRT_2`: the constant's own rounding.
const FRAC_1_SQRT_2_LOW: f64 = -4.833_646_656_726_457e-17;

/// `φ(x) = exp(−x²/2)/√(2π)`. `x²` rounds by up to `x²·ε/2`, which `exp` turns into a relative
/// error of `x²/2` of it; its rounding, from a fused multiply-add, puts that back to first order.
pub(crate) fn pdf(x: f64) -> f64 {
    let square = x * x;
    // An infinite square leaves no rounding to take back, and `∞ − ∞` would make it NaN.
    let square_low = if square.is_finite() {
        x.mul_add(x, -square)
    } else {
        0.0
    };
    (-0.5 * square).libm_exp() * (1.0 - 0.5 * square_low) / (2.0 * PI).sqrt()
}

/// `Φ(x) = ½·erfc(−x/√2)`, which keeps its relative precision in the lower tail, where
/// `½·(1 + erf(x/√2))` cancels. `x/√2` rounds in the constant and in the product, by `δ`, and
/// `erfc` turns that into a relative error of `2·(x/√2)²` of it, 100ε at −10; `√2·φ(x)·δ`, the
/// first-order term of `erfc` at `x/√2 + δ`, puts it back, as a double-double argument would.
pub(crate) fn cdf(x: f64) -> f64 {
    let z = x * FRAC_1_SQRT_2;
    let delta = if z.is_finite() {
        x.mul_add(FRAC_1_SQRT_2, -z) + x * FRAC_1_SQRT_2_LOW
    } else {
        0.0
    };
    0.5 * (-z).libm_erfc() + SQRT_2 * pdf(x) * delta
}

/// `Φ⁻¹(p)` for `p` in `(0, 1)`, by Wichura's AS 241 (*Applied Statistics* 37, 1988), which R's
/// `qnorm` takes: about 1e-16 relative, a rational form in `(p − ½)²` over the centre and in
/// `√(−ln p)` over each tail.
///
/// # Panics
///
/// Unless `p` is in `(0, 1)`: a quantile of 0 or 1 is infinite, which no caller asks for.
pub(crate) fn quantile(p: f64) -> f64 {
    assert!(p > 0.0, "a normal quantile of {p}");
    assert!(p < 1.0, "a normal quantile of {p}");
    let q = p - 0.5;
    if q.abs() <= 0.425 {
        let r = 0.180_625 - q * q;
        return q * horner(&CENTRAL_P, r) / (horner(&CENTRAL_Q, r) * r + 1.0);
    }
    let tail = if q < 0.0 { p } else { 1.0 - p };
    let r = (-tail.libm_ln()).sqrt();
    let magnitude = if r <= 5.0 {
        let r = r - 1.6;
        horner(&NEAR_P, r) / (horner(&NEAR_Q, r) * r + 1.0)
    } else {
        let r = r - 5.0;
        horner(&FAR_P, r) / (horner(&FAR_Q, r) * r + 1.0)
    };
    if q < 0.0 { -magnitude } else { magnitude }
}

/// `c₀·xⁿ + … + cₙ`, by Horner's rule, as AS 241 evaluates each form.
fn horner(coefficients: &[f64], x: f64) -> f64 {
    coefficients[1..]
        .iter()
        .fold(coefficients[0], |sum, &coefficient| sum * x + coefficient)
}

#[cfg(test)]
mod tests;
