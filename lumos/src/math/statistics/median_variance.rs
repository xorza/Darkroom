//! The small-sample variance of the median of normal samples.

/// The largest sample count [`median_variance_factor`] answers for.
pub(crate) const MAX_SAMPLES: usize = 32;

/// `n · Var(median)` of `n` unit-normal samples, `n` from 1, to nine digits: the factor a mean's
/// variance `σ²/n` is multiplied by to give the median's. 1 for one or two samples, whose median
/// is their mean, rising toward `π/2` with the odd counts above the even ones: an odd median is one
/// order statistic, an even one the mean of two. By quadrature of the order statistics' densities,
/// the middle one's for an odd `n` and the joint density of the middle two for an even one
/// (Kenney & Keeping, *Mathematics of Statistics* II, §7.2); 1.346 at 3 and 1.193 at 4, as their
/// tables give.
const FACTORS: [f64; MAX_SAMPLES] = [
    1.000000000,
    1.000000000,
    1.346013314,
    1.192798481,
    1.434168308,
    1.288456025,
    1.473128031,
    1.345446895,
    1.494911532,
    1.383264451,
    1.508786769,
    1.410194387,
    1.518386935,
    1.430349916,
    1.525419781,
    1.446003651,
    1.530791884,
    1.458513532,
    1.535028526,
    1.468740996,
    1.538454855,
    1.477258944,
    1.541282814,
    1.484463248,
    1.543656426,
    1.490636252,
    1.545676960,
    1.495984757,
    1.547417690,
    1.500663711,
    1.548932941,
    1.504791509,
];

/// `n · Var(median)` of `n` unit-normal samples — see [`FACTORS`].
///
/// # Panics
/// When `n` is 0 or past [`MAX_SAMPLES`].
pub(crate) fn median_variance_factor(n: usize) -> f64 {
    assert!(
        (1..=MAX_SAMPLES).contains(&n),
        "a median variance factor of {n} samples"
    );
    FACTORS[n - 1]
}

#[cfg(test)]
mod tests {
    use common::Libm;

    use crate::math::normal;
    use crate::math::statistics::median_variance::median_variance_factor;

    /// The odd factors again, by Simpson quadrature of the middle order statistic's density,
    /// `n!/(k!)² Φᵏ(1 − Φ)ᵏ φ`, over ±10σ, where the tails hold nothing an f64 keeps; and the
    /// counts of one and two, whose median is a mean.
    #[test]
    fn the_odd_factors_are_the_middle_order_statistics_variance() {
        assert_eq!(median_variance_factor(1), 1.0);
        assert_eq!(median_variance_factor(2), 1.0);
        let steps = 20_000u32;
        let h = 20.0 / f64::from(steps);
        for n in (3..=31).step_by(2) {
            let k = (n - 1) / 2;
            let ln_factorial = |m: usize| (1..=m).map(|i| (i as f64).libm_ln()).sum::<f64>();
            let ln_coefficient = ln_factorial(n) - 2.0 * ln_factorial(k);
            let density = |x: f64| {
                let cdf = normal::cdf(x);
                let ln_mass = ln_coefficient + k as f64 * (cdf * (1.0 - cdf)).libm_ln();
                x * x * ln_mass.libm_exp() * normal::pdf(x)
            };
            let simpson = (0..=steps)
                .map(|i| {
                    let weight = match i {
                        0 => 1.0,
                        i if i == steps => 1.0,
                        i if i % 2 == 1 => 4.0,
                        _ => 2.0,
                    };
                    weight * density(-10.0 + f64::from(i) * h)
                })
                .sum::<f64>()
                * h
                / 3.0;
            let factor = n as f64 * simpson;
            assert!(
                (factor - median_variance_factor(n)).abs() < 1e-8,
                "{n}: {factor} against {}",
                median_variance_factor(n)
            );
        }
    }
}
