#![expect(
    clippy::excessive_precision,
    reason = "each reference as solved to more digits than an f64 holds, so it reads against its source"
)]

use common::Libm;
use statrs::distribution::{Continuous, ContinuousCDF, Normal};

use crate::math::normal;

/// `got` within `ulps` of `want`'s ε, relative.
fn near(got: f64, want: f64, ulps: f64) -> bool {
    (got - want).abs() <= ulps * f64::EPSILON * want.abs()
}

/// The anchors: `Φ(0) = ½` and `Φ⁻¹(½) = 0` exactly; `φ(0) = 1/√(2π)` within ε. Against values
/// solved to 120 digits and more from the Maclaurin series of `erf`: `Φ⁻¹` of the `f64` nearest
/// 0.975, `0.97499999999999997780`, is `1.95996398454005385560`, within 2ε, and its mirror at
/// 0.025, a different `f64`, within 8ε. `Φ` down to −20, deep in the tail where `½·(1 + erf)`
/// would cancel to 0, and `φ` out to 30, each within 2ε; at the infinities and past `x²`'s overflow
/// their limits, which no rounding correction turns into NaN.
#[test]
fn the_anchors_hold() {
    assert_eq!(normal::cdf(0.0), 0.5);
    assert_eq!(normal::quantile(0.5), 0.0);
    let pdf_zero = 0.398_942_280_401_432_7;
    assert!(
        near(normal::pdf(0.0), pdf_zero, 1.0),
        "{}",
        normal::pdf(0.0)
    );
    let central = 1.959_963_984_540_053_856;
    let got = normal::quantile(0.975);
    assert!(near(got, central, 2.0), "{got}");
    let got = normal::quantile(0.025);
    assert!(near(got, -central, 8.0), "{got}");
    for (x, want) in [
        (-1.0, 1.586_552_539_314_570_514e-1),
        (-3.0, 1.349_898_031_630_094_527e-3),
        (-5.0, 2.866_515_718_791_939_117e-7),
        (-10.0, 7.619_853_024_160_526_066e-24),
        (-20.0, 2.753_624_118_606_233_695e-89),
    ] {
        let got = normal::cdf(x);
        assert!(near(got, want, 2.0), "Φ({x}) = {got:e}");
    }
    assert_eq!(normal::cdf(f64::NEG_INFINITY), 0.0);
    assert_eq!(normal::cdf(f64::INFINITY), 1.0);
    assert_eq!(normal::pdf(f64::INFINITY), 0.0);
    assert_eq!(normal::pdf(-1e200), 0.0);
    for (x, want) in [
        (3.0, 4.431_848_411_938_007_176e-3),
        (10.0, 7.694_598_626_706_419_346e-23),
        (30.0, 1.473_646_134_878_547_519e-196),
    ] {
        let got = normal::pdf(x);
        assert!(near(got, want, 2.0), "φ({x}) = {got:e}");
    }
}

/// Against statrs, the oracle: the quantile within 4ε over a grid of `p` from 1e-4 to 0.9999 and
/// the lower tails down to 1e-300, which take each of AS 241's three forms; the density within
/// 32ε on `[−8, 8]`. Not the cdf: statrs's own `erfc` errs by 8.6e-11 relative at −1, so the
/// anchors above hold it.
#[test]
fn each_function_matches_statrs() {
    let reference = Normal::standard();
    let relative = |got: f64, want: f64| {
        if want == 0.0 {
            got.abs()
        } else {
            (got - want).abs() / want.abs()
        }
    };
    let mut ps: Vec<f64> = (1..10_000).map(|i| f64::from(i) / 10_000.0).collect();
    ps.extend((1..=300).map(|e| 10f64.libm_powi(-e)));
    for &p in &ps {
        let error = relative(normal::quantile(p), reference.inverse_cdf(p));
        assert!(error <= 4.0 * f64::EPSILON, "Φ⁻¹({p}): {error:e} off");
    }
    for i in -800..=800 {
        let x = f64::from(i) / 100.0;
        let error = relative(normal::pdf(x), reference.pdf(x));
        assert!(error <= 32.0 * f64::EPSILON, "φ({x}): {error:e} off");
    }
}

/// `Φ(Φ⁻¹(p)) = p` across the centre and both tails, within `8ε·max(1, x²)` relative at
/// `x = Φ⁻¹(p)`: in the tail `d ln Φ / dx ≈ |x|`, so `p` carries the quantile's relative error
/// times `x²`.
#[test]
fn the_quantile_inverts_the_cdf() {
    for p in [1e-200, 1e-20, 1e-6, 0.01, 0.2, 0.5, 0.7, 0.99, 1.0 - 1e-6] {
        let x = normal::quantile(p);
        let back = normal::cdf(x);
        let bound = 8.0 * f64::EPSILON * (x * x).max(1.0) * p;
        assert!((back - p).abs() <= bound, "{p}: {back}");
    }
}
