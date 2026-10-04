//! [`PixelQuadrature`]: Gauss–Legendre quadrature over one pixel.

/// The highest order a [`PixelQuadrature`] holds.
pub(crate) const MAX_ORDER: usize = 16;

/// The Gauss–Legendre nodes and weights of one order over `[−½, ½]`, the span of a pixel about its
/// centre. A tensor product of two integrates a profile over a pixel: `Σᵢ Σⱼ wᵢ·wⱼ·f(x + ξᵢ, y + ξⱼ)`
/// is exact for every polynomial of degree `2·order − 1` in each coordinate, and the weights sum to
/// one, so the result is the pixel's mean.
///
/// The nodes are the roots of the Legendre polynomial `Pₙ`, found by Newton's method from
/// Tricomi's estimate `cos(π(i − ¼)/(n + ½))`; the weights are `2/((1 − x²)·Pₙ′(x)²)`, both halved
/// onto the pixel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelQuadrature {
    order: usize,
    nodes: [f64; MAX_ORDER],
    weights: [f64; MAX_ORDER],
}

/// How a profile's analytic continuation grows off the real axis, which bounds the error of
/// integrating it over a pixel. Both have their peak at 1.
#[derive(Debug, Clone, Copy)]
pub(crate) enum AnalyticProfile {
    /// A Gaussian whose σ along each pixel axis, at any fixed offset along the other, is at least
    /// `sigma`: on `|Im t| ≤ b` its modulus is at most `e^(b²/2σ²)`.
    Gaussian { sigma: f64 },
    /// `(1 + r²/α²)^−β`: analytic on `|Im t| < α`, where the base keeps a real part of at least
    /// `1 − b²/α²` along either axis, so the modulus is at most `(1 − b²/α²)^−β`.
    Moffat { alpha: f64, beta: f64 },
}

impl AnalyticProfile {
    /// `ln` of the largest modulus on the strip `|Im t| ≤ b`, infinite where the strip reaches a
    /// singularity.
    fn log_modulus(self, b: f64) -> f64 {
        match self {
            Self::Gaussian { sigma } => b * b / (2.0 * sigma * sigma),
            Self::Moffat { alpha, beta } => {
                let reach = b / alpha;
                if reach < 1.0 {
                    -beta * (1.0 - reach * reach).ln()
                } else {
                    f64::INFINITY
                }
            }
        }
    }
}

/// `Pₙ(x)` and `Pₙ′(x)` at one point.
#[derive(Debug, Clone, Copy)]
struct Legendre {
    value: f64,
    slope: f64,
}

impl Legendre {
    /// By Bonnet's recurrence `k·Pₖ = (2k − 1)·x·Pₖ₋₁ − (k − 1)·Pₖ₋₂`, and the slope from
    /// `(x² − 1)·Pₙ′ = n·(x·Pₙ − Pₙ₋₁)`, which holds inside `(−1, 1)` where every root lies.
    fn at(order: usize, x: f64) -> Self {
        let (mut previous, mut value) = (1.0, x);
        for k in 2..=order {
            let k = k as f64;
            let next = ((2.0 * k - 1.0) * x * value - (k - 1.0) * previous) / k;
            previous = value;
            value = next;
        }
        Self {
            value,
            slope: order as f64 * (x * value - previous) / (x * x - 1.0),
        }
    }
}

impl PixelQuadrature {
    /// Newton converges quadratically from Tricomi's estimate, to the root's rounding within
    /// five steps up to order 16; the cap only stops a step that oscillates by an ulp.
    const MAX_NEWTON_STEPS: usize = 16;

    /// Golden-section steps over `ln ρ`: each keeps 0.618 of the interval, so 64 narrow it by
    /// 4e-14 of its width, past where the bound changes.
    const GOLDEN_STEPS: usize = 64;

    /// The smallest order whose error on a pixel stays within `tolerance` of the profile's peak.
    ///
    /// Along one axis the mean over `[−½, ½]` is half the integral over `[−1, 1]` of `f(s/2)`, and
    /// Gauss quadrature of `n` points errs there by at most `(64/15)·M·ρ^(2 − 2n)/(ρ² − 1)` for
    /// `f` analytic inside the Bernstein ellipse `E_ρ` with modulus at most `M` (Trefethen,
    /// *Approximation Theory and Approximation Practice*, theorem 19.3, whose rule of `n + 1`
    /// points this restates). Scaled onto the pixel, `E_ρ` reaches `b = (ρ − 1/ρ)/4` off the
    /// axis, and the error halves. The bound holds for every ρ, its logarithm is convex in `ln ρ`,
    /// and golden-section search finds the least. The tensor rule's error is at most the sum of
    /// the two axes' — `(I − Qₓ)·I_y` and `Qₓ·(I − Q_y)`, positive weights summing to 1 — so each
    /// axis takes half the tolerance. `None` past [`MAX_ORDER`].
    pub(crate) fn sufficient_order(profile: AnalyticProfile, tolerance: f64) -> Option<usize> {
        let log_tolerance = (tolerance / 2.0).ln();
        (1..=MAX_ORDER).find(|&order| Self::log_error_bound(profile, order) <= log_tolerance)
    }

    /// `ln` of the least of the one-axis bound over ρ, for `order` points.
    fn log_error_bound(profile: AnalyticProfile, order: usize) -> f64 {
        let n = order as f64;
        let log_bound = |log_rho: f64| {
            let rho = log_rho.exp();
            let b = (rho - 1.0 / rho) / 4.0;
            (32.0f64 / 15.0).ln() + profile.log_modulus(b) + (2.0 - 2.0 * n) * log_rho
                - (rho * rho - 1.0).ln()
        };
        // ρ = 1 + 1e-9 and ρ = 1e6 bracket every least: below, the `ρ² − 1` term grows without
        // bound; beyond, the modulus does, or the ellipse meets the singularity.
        let (mut low, mut high) = (1e-9f64, 1e6f64.ln());
        let ratio = (5f64.sqrt() - 1.0) / 2.0;
        let mut left = high - ratio * (high - low);
        let mut right = low + ratio * (high - low);
        let (mut at_left, mut at_right) = (log_bound(left), log_bound(right));
        for _ in 0..Self::GOLDEN_STEPS {
            if at_left <= at_right {
                high = right;
                right = left;
                at_right = at_left;
                left = high - ratio * (high - low);
                at_left = log_bound(left);
            } else {
                low = left;
                left = right;
                at_left = at_right;
                right = low + ratio * (high - low);
                at_right = log_bound(right);
            }
        }
        at_left.min(at_right)
    }

    /// The rule of `order` points, ascending.
    pub(crate) fn gauss_legendre(order: usize) -> Self {
        assert!(
            (1..=MAX_ORDER).contains(&order),
            "a pixel quadrature has 1 to {MAX_ORDER} points, not {order}"
        );
        let mut rule = Self {
            order,
            nodes: [0.0; MAX_ORDER],
            weights: [0.0; MAX_ORDER],
        };
        let n = order as f64;
        // The roots pair as ±x, and an odd order adds 0; each pair is found once, from the right.
        for i in 0..order.div_ceil(2) {
            let mut x = (std::f64::consts::PI * (i as f64 + 0.75) / (n + 0.5)).cos();
            for _ in 0..Self::MAX_NEWTON_STEPS {
                let legendre = Legendre::at(order, x);
                let step = legendre.value / legendre.slope;
                x -= step;
                if step.abs() <= f64::EPSILON * x.abs().max(f64::EPSILON) {
                    break;
                }
            }
            if order % 2 == 1 && i == order / 2 {
                x = 0.0;
            }
            let slope = Legendre::at(order, x).slope;
            let weight = 1.0 / ((1.0 - x * x) * slope * slope);
            let (low, high) = (i, order - 1 - i);
            rule.nodes[low] = -x / 2.0;
            rule.nodes[high] = x / 2.0;
            rule.weights[low] = weight;
            rule.weights[high] = weight;
        }
        rule
    }

    pub(crate) fn nodes(&self) -> &[f64] {
        &self.nodes[..self.order]
    }

    pub(crate) fn weights(&self) -> &[f64] {
        &self.weights[..self.order]
    }

    /// The mean of `f` over the pixel centred at `(x, y)`: rows of nodes outer, each term
    /// `wₓ·w_y` fused into the sum, the order the fits' vector kernels sum in.
    pub(crate) fn integrate(&self, x: f64, y: f64, f: impl Fn(f64, f64) -> f64) -> f64 {
        let mut sum = 0.0f64;
        for (&dy, &wy) in self.nodes().iter().zip(self.weights()) {
            for (&dx, &wx) in self.nodes().iter().zip(self.weights()) {
                sum = (wx * wy).mul_add(f(x + dx, y + dy), sum);
            }
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::pixel_gaussian::PixelGaussian;

    /// The bound holds where it is checked against the truth: a Gaussian's pixel mean, in closed
    /// form, at σ 0.5, 1 and 2 and 41 phases from the centre to 4σ, for every order through 8, errs
    /// by no more than the bound of the order, and 4ε for the rounding of the sums and the closed
    /// form, which the bound passes under from order 8. A Moffat's, against a 16-point rule,
    /// likewise. The orders it picks for the f32 tolerance
    /// 2⁻²⁵ at the narrowest profiles the fits admit are 7 and 11.
    #[test]
    fn the_bound_holds_and_picks_the_orders() {
        for sigma in [0.5, 1.0, 2.0] {
            let gaussian = PixelGaussian { sigma };
            for order in 1..=8 {
                let rule = PixelQuadrature::gauss_legendre(order);
                let bound =
                    PixelQuadrature::log_error_bound(AnalyticProfile::Gaussian { sigma }, order)
                        .exp();
                for step in 0..=40 {
                    let d = 4.0 * sigma * f64::from(step) / 40.0;
                    let quadrature =
                        rule.integrate(d, 0.0, |t, _| (-t * t / (2.0 * sigma * sigma)).exp());
                    let error = (quadrature - gaussian.mean_at(d)).abs();
                    assert!(
                        error <= bound + 4.0 * f64::EPSILON,
                        "σ {sigma}, order {order}, d {d}: {error} > {bound}"
                    );
                }
            }
        }
        let (alpha, beta) = (0.8, 1.5);
        let reference = PixelQuadrature::gauss_legendre(MAX_ORDER);
        let moffat = |t: f64, _| (1.0 + t * t / (alpha * alpha)).powf(-beta);
        for order in 1..=8 {
            let rule = PixelQuadrature::gauss_legendre(order);
            let bound =
                PixelQuadrature::log_error_bound(AnalyticProfile::Moffat { alpha, beta }, order)
                    .exp();
            for step in 0..=40 {
                let d = 3.0 * f64::from(step) / 40.0;
                let error =
                    (rule.integrate(d, 0.0, moffat) - reference.integrate(d, 0.0, moffat)).abs();
                assert!(
                    error <= bound + 4.0 * f64::EPSILON,
                    "Moffat, order {order}, d {d}: {error} > {bound}"
                );
            }
        }
        let tolerance = 2f64.powi(-25);
        assert_eq!(
            PixelQuadrature::sufficient_order(AnalyticProfile::Gaussian { sigma: 0.5 }, tolerance),
            Some(7)
        );
        let narrowest = 2.0 * 0.5 * (2.0 * std::f64::consts::LN_2).sqrt()
            / (2.0 * (2f64.powf(1.0 / 1.01) - 1.0).sqrt());
        assert_eq!(
            PixelQuadrature::sufficient_order(
                AnalyticProfile::Moffat {
                    alpha: narrowest,
                    beta: 1.01
                },
                tolerance
            ),
            Some(11)
        );
    }

    /// Orders 2 and 3 in closed form, halved onto the pixel: ±1/(2√3) at ½ each; and 0 at 4/9 with
    /// ±√(3/5)/2 at 5/18 each.
    #[test]
    fn low_orders_match_their_closed_forms() {
        let two = PixelQuadrature::gauss_legendre(2);
        let node = 1.0 / (2.0 * 3f64.sqrt());
        assert!((two.nodes()[1] - node).abs() <= f64::EPSILON);
        assert_eq!(two.nodes()[0], -two.nodes()[1]);
        assert!(
            two.weights()
                .iter()
                .all(|&w| (w - 0.5).abs() <= f64::EPSILON)
        );

        let three = PixelQuadrature::gauss_legendre(3);
        assert_eq!(three.nodes()[1], 0.0);
        assert!((three.nodes()[2] - 0.6f64.sqrt() / 2.0).abs() <= f64::EPSILON);
        assert!((three.weights()[1] - 4.0 / 9.0).abs() <= f64::EPSILON);
        assert!((three.weights()[0] - 5.0 / 18.0).abs() <= f64::EPSILON);
    }

    /// Every order integrates `xᵏ` exactly to degree `2n − 1`: `∫ xᵏ dx` over `[−½, ½]` is 0 for an
    /// odd `k` and `2⁻ᵏ/(k + 1)` for an even one. The nodes ascend and the weights sum to one. A
    /// sum of at most 16 terms below 1 rounds within 16 ε, and each node errs by an ε of its own.
    #[test]
    fn each_order_is_exact_to_its_degree() {
        for order in 1..=MAX_ORDER {
            let rule = PixelQuadrature::gauss_legendre(order);
            assert!(rule.nodes().windows(2).all(|pair| pair[0] < pair[1]));
            for k in 0..2 * order as i32 {
                let quadrature: f64 = rule
                    .nodes()
                    .iter()
                    .zip(rule.weights())
                    .map(|(&x, &w)| w * x.powi(k))
                    .sum();
                let exact = if k % 2 == 1 {
                    0.0
                } else {
                    0.5f64.powi(k) / f64::from(k + 1)
                };
                assert!(
                    (quadrature - exact).abs() <= 32.0 * f64::EPSILON,
                    "order {order}, x^{k}: {quadrature} vs {exact}"
                );
            }
        }
    }
}
