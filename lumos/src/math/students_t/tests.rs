#![expect(
    clippy::excessive_precision,
    reason = "each reference as solved to more digits than an f64 holds, so it reads against its source"
)]

use std::f64::consts::FRAC_2_PI;
use std::num::NonZeroU32;

use common::Libm;
use statrs::distribution::{ContinuousCDF, StudentsT};

use crate::math::students_t::{Point, density_scale, upper_quantile};

fn df(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}

/// `P(|T| > t)` at `n` degrees, out of its log.
fn two_sided_tail(n: u32, t: f64) -> f64 {
    Point::new(f64::from(n), t.libm_ln())
        .ln_two_sided_tail(density_scale(n).libm_ln())
        .libm_exp()
}

/// The closed forms, exact within an ulp: at one degree `cot(π/4) = 1` and `cot(π·10⁻¹²)`, at two
/// `√(2/3)` for a tail of ¼. At three, `3.18244630528370959…`, the root of the closed cdf
/// `½ + (t√3/(3 + t²) + atan(t/√3))/π = 0.975` to 60 digits, within 2 ulps.
#[test]
fn the_closed_forms_and_a_root_at_three_degrees_hold() {
    let near =
        |got: f64, want: f64, ulps: f64| (got - want).abs() <= ulps * f64::EPSILON * want.abs();
    assert!(near(upper_quantile(df(1), 0.25), 1.0, 1.0));
    let tiny = 1e-12;
    let cot = 1.0 / (std::f64::consts::PI * tiny).libm_tan();
    assert_eq!(upper_quantile(df(1), tiny), cot);
    assert!(near(
        upper_quantile(df(2), 0.25),
        (2.0f64 / 3.0).sqrt(),
        1.0
    ));
    let three = upper_quantile(df(3), 0.025);
    assert!(near(three, 3.182_446_305_283_709_6, 2.0), "{three}");
}

/// The two-sided tail against its closed forms: `(2/π)·atan(1/t)` at one degree and
/// `1 − t/√(2 + t²) = 2/(s·(s + t))`, `s = √(2 + t²)`, at two, without its cancellation; each
/// through the continued fraction's side and the complement's. Within `(8 + |ln P|)·ε`: the tail
/// comes out of its log, whose rounding `exp` scales by `|ln P|`.
#[test]
fn the_two_sided_tail_takes_its_closed_forms() {
    for t in [0.1, 0.5, 1.0, 1.7, 4.0, 30.0, 1e4] {
        let one = two_sided_tail(1, t);
        let want = FRAC_2_PI * (1.0 / t).libm_atan();
        let bound = (8.0 + want.libm_ln().abs()) * f64::EPSILON * want;
        assert!((one - want).abs() <= bound, "1 df, {t}: {one} vs {want}");
        let two = two_sided_tail(2, t);
        let root = (2.0 + t * t).sqrt();
        let want = 2.0 / (root * (root + t));
        let bound = (8.0 + want.libm_ln().abs()) * f64::EPSILON * want;
        assert!((two - want).abs() <= bound, "2 df, {t}: {two} vs {want}");
    }
}

/// The quantile's tail comes back from the two-sided tail's log within 1e-13 of `ln(2·tail)`, or
/// of 1 where that is smaller, from 10⁻³⁰⁰ to 0.45 and from 3 to 2000 degrees: Newton's steps
/// stop on `ln t`, and the log is what they hold. Against statrs within 1e-12 up to 100 degrees
/// where statrs's `1 − tail` keeps the digits, tails of 0.01 and up; past that statrs itself errs
/// by 1e-12, as the anchors below show.
#[test]
fn the_quantile_inverts_the_tail_and_matches_statrs() {
    for n in [3, 4, 5, 7, 10, 30, 100, 500, 2000] {
        for tail in [
            1e-300, 1e-30, 1e-12, 1e-6, 1e-3, 0.01, 0.025, 0.1, 0.3, 0.45,
        ] {
            let t = upper_quantile(df(n), tail);
            let back =
                Point::new(f64::from(n), t.libm_ln()).ln_two_sided_tail(density_scale(n).libm_ln());
            let want = (2.0 * tail).libm_ln();
            let bound = 1e-13 * want.abs().max(1.0);
            assert!(
                (back - want).abs() <= bound,
                "{n} df, {tail}: t {t}, {back} vs {want}"
            );
            if tail >= 0.01 && n <= 100 {
                let want = StudentsT::new(0.0, 1.0, f64::from(n))
                    .unwrap()
                    .inverse_cdf(1.0 - tail);
                assert!(
                    (t - want).abs() <= 1e-12 * want,
                    "{n} df, {tail}: {t} vs {want}"
                );
            }
        }
    }
}

/// A tail far past where the density underflows still places its quantile: at three degrees
/// `P(T > t) ≈ (2√3/π)·t⁻³` far out, so a tail of 10⁻³⁰⁰ sits near `t = 10¹⁰⁰·(2√3/π)^⅓`, and its
/// log comes back from the two-sided tail's log within 1e-13.
#[test]
fn a_tail_past_the_density_underflow_places_its_quantile() {
    let tail = 1e-300;
    let t = upper_quantile(df(3), tail);
    let far = 1e100 * (2.0 * 3.0f64.sqrt() / std::f64::consts::PI).libm_cbrt();
    assert!((t - far).abs() <= 1e-12 * far, "{t} vs {far}");
    let ln_both = Point::new(3.0, t.libm_ln()).ln_two_sided_tail(density_scale(3).libm_ln());
    let want = (2.0 * tail).libm_ln();
    assert!(
        (ln_both - want).abs() <= 1e-13 * want.abs(),
        "{ln_both} vs {want}"
    );
}

/// At many degrees, against roots of the closed cdf at even `ν`, `½·(1 − sin θ·Σₖ (2k−1)!!/(2k)!!
/// cos^{2k} θ)` over `k < ν/2` (Abramowitz & Stegun 26.7.4), solved to 50 digits: within 8ε, where
/// 4.2ε is the worst measured; statrs is 1.03e-12 off the first.
#[test]
fn the_quantile_holds_at_many_degrees() {
    for (n, tail, want) in [
        (500, 0.45, 0.125_725_186_671_952_886),
        (500, 0.001, 3.106_611_624_321_628_773),
        (2000, 1e-6, 4.767_480_145_393_622_168),
    ] {
        let t = upper_quantile(df(n), tail);
        assert!(
            (t - want).abs() <= 8.0 * f64::EPSILON * want,
            "{n} df, {tail}: {t} vs {want}"
        );
    }
}
