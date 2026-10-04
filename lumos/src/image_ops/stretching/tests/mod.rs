#[cfg(feature = "real-data")]
mod real_data;

use crate::image_ops::stretching::*;
use crate::internals::images::{gray_image as gray, rgb_image as rgb};
use crate::internals::prelude::*;
use crate::math::statistics::median_mut;
use std::f32::consts::LN_10;
use std::iter;

fn median_of(v: &[f32]) -> f32 {
    let mut c = v.to_vec();
    median_mut(&mut c)
}

/// How far f32 `MTF(m, x) = (m − 1)x / ((2m − 1)x − m)` can sit from its exact value `v`: two
/// roundings above the line, three in the denominator — which cancels by
/// `κ = (|2m − 1|·x + m) / |(2m − 1)x − m|` — and the division, half an ulp each: `(3κ + 3)·ε/2`.
fn mtf_bound(m: f32, x: f32, v: f32) -> f32 {
    let kappa = ((2.0 * m - 1.0).abs() * x + m) / ((2.0 * m - 1.0) * x - m).abs();
    (3.0 * kappa + 3.0) * f32::EPSILON / 2.0 * v
}

/// The fixed points, the identity and the direction: `MTF(m, 0) = 0` and `MTF(m, 1) = 1` take
/// their own branches; `MTF(m, m) = (m − 1)m / (2m(m − 1)) = ½`; at `m = ½` the denominator is
/// `−½` and `MTF = x` exactly; and at x = ¼, `m = ¾` darkens to `−1/16 / −5/8 = 0.1`, every operand
/// exact and the quotient rounded once.
#[test]
fn mtf_fixed_points_identity_and_direction() {
    for &m in &[0.1f32, 0.25, 0.5, 0.75, 0.9] {
        assert_eq!(mtf(m, 0.0), 0.0, "MTF(m,0) = 0");
        assert_eq!(mtf(m, 1.0), 1.0, "MTF(m,1) = 1");
        assert_close!(
            mtf(m, m),
            0.5,
            mtf_bound(m, m, 0.5),
            "MTF(m,m) = 0.5 for m={m}"
        );
    }
    for &x in &[0.1f32, 0.3, 0.7, 0.9] {
        assert_eq!(mtf(0.5, x), x, "MTF(0.5,·) is the identity");
    }
    assert_eq!(mtf(0.75, 0.25), 0.1, "m > 0.5 darkens");
}

#[test]
fn mtf_monotonic_increasing() {
    for &m in &[0.1f32, 0.3, 0.5, 0.7, 0.9] {
        let mut prev = f32::NEG_INFINITY;
        for i in 0..=100 {
            let y = mtf(m, i as f32 / 100.0);
            assert!(y >= prev - 1e-7, "MTF must be monotonic (m={m})");
            prev = y;
        }
    }
}

/// The midtones balance that maps `x0` onto `t` is `MTF(t, x0)`: `MTF(MTF(t, x0), x0) = t`. The
/// identity holds in f64 to its rounding; the f32 composition is the f64 one at the f32 `m` to
/// [`mtf_bound`].
#[test]
fn mtf_self_inverse_identity() {
    let mtf64 = |m: f64, x: f64| ((m - 1.0) * x) / ((2.0 * m - 1.0) * x - m);
    for &x0 in &[0.02f32, 0.05, 0.1, 0.3] {
        for &t in &[0.1f32, 0.25, 0.4] {
            let (x, target) = (f64::from(x0), f64::from(t));
            assert_close!(
                mtf64(mtf64(target, x), x),
                target,
                1e-14,
                "f64, x0={x0}, t={t}"
            );
            let m = mtf(t, x0);
            let exact = mtf64(f64::from(m), x);
            let got = mtf(m, x0);
            assert_close!(got, exact, mtf_bound(m, x0, got), "x0={x0}, t={t}");
        }
    }
}

#[test]
fn asinh_endpoints_and_monotonic() {
    for &beta in &[0.01f32, 0.1, 1.0, 10.0] {
        let c = AsinhCurve::new(beta);
        // asinh(0) = 0 exactly; asinh(1/β) times its own reciprocal rounds twice.
        assert_eq!(c.eval(0.0), 0.0, "f(0) = 0 (beta={beta})");
        assert_close!(c.eval(1.0), 1.0, f32::EPSILON, "f(1) = 1 (beta={beta})");
        let mut prev = f32::NEG_INFINITY;
        for i in 0..=100 {
            let y = c.eval(i as f32 / 100.0);
            // x/β, asinhf, the scale and the clamp are each monotone, rounding included.
            assert!(y >= prev, "asinh stretch must be monotonic (beta={beta})");
            prev = y;
        }
    }
}

/// The curve `asinh(x/β) / asinh(1/β)` in f64, at the f32 `β` the curve was built from.
fn asinh_reference(beta: f32, x: f32) -> f64 {
    let beta = f64::from(beta);
    (f64::from(x) / beta).asinh() / (1.0 / beta).asinh()
}

/// The f32 curve rounds `1/β` and `x/β` (2ε on the argument, which `asinh` does not magnify),
/// takes libm's `asinhf` twice (ε each), the reciprocal of the norm and the product (ε each): 7ε.
const ASINH_EVAL_BOUND: f32 = 7.0 * f32::EPSILON;

/// A smaller `β` lifts a faint value higher, and a large one nears the identity — at β = 1000,
/// `x(1 + (1 − x²)/(6β²))` is 0.5 + 6e-8 at 0.5, inside one ulp. Hand-computed at β = 0.01:
/// `f(0.1) = asinh(10)/asinh(100) = 2.998223/5.298342 = 0.565879`.
#[test]
fn asinh_beta_controls_strength() {
    let aggressive = AsinhCurve::new(0.01).eval(0.05);
    let gentle = AsinhCurve::new(1.0).eval(0.05);
    assert!(
        aggressive > gentle,
        "smaller beta lifts faint signal more ({aggressive} vs {gentle})"
    );
    for (beta, x) in [(0.01f32, 0.05f32), (1.0, 0.05), (1000.0, 0.5), (0.01, 0.1)] {
        let expected = asinh_reference(beta, x);
        let got = AsinhCurve::new(beta).eval(x);
        assert!(
            (f64::from(got) - expected).abs() <= f64::from(ASINH_EVAL_BOUND) * expected,
            "beta {beta}, x {x}: {got} vs {expected}"
        );
    }
    assert!((asinh_reference(1000.0, 0.5) - 0.5).abs() < 1e-7);
    assert!((asinh_reference(0.01, 0.1) - 0.565_879).abs() < 1e-6);
}

/// Every reachable median lands on its target, down to the near-zero sky a background
/// subtraction leaves. The bisection ends on adjacent f32 exponents `m = log₁₀ β` that bracket the
/// target to the ~5ε its `g` rounds to; `|d ln g / d ln β| ≤ 1`, so one ulp of `m` moves `g` by at
/// most `ln 10 · ulp(m)` relative. `10^m` rounds once more (ε), and the curve evaluated here
/// differs from `g` by [`ASINH_EVAL_BOUND`]: together `ln 10 · ulp(m) + 13ε` relative.
#[test]
fn solve_beta_hits_target_background() {
    for &(median, target) in &[
        (0.05f32, 0.2f32),
        (0.1, 0.25),
        (0.02, 0.15),
        (1e-5, 0.2),
        (1e-3, 0.2),
    ] {
        let beta = solve_asinh_beta(median, target).unwrap();
        let got = AsinhCurve::new(beta).eval(median);
        let exponent = beta.log10().abs();
        let ulp = exponent.next_up() - exponent;
        let bound = (LN_10 * ulp + 13.0 * f32::EPSILON) * target;
        assert!(
            (got - target).abs() <= bound,
            "median {median} -> {got}, want {target} within {bound:e}"
        );
    }
}

/// A background at or below zero gives a brightening curve nothing to lift, and a median at or
/// above the target cannot be brought down by one: both auto stretches report it instead of
/// returning a curve that silently misses the target.
#[test]
fn auto_stretches_report_an_unreachable_background() {
    for median in [0.0f32, -0.001, 0.25, 0.9] {
        assert!(
            solve_asinh_beta(median, 0.2).is_none(),
            "asinh: median {median}"
        );
    }
    // Both auto curves fail at each check of their black point: at white (1.5 − 1.5σ is past 1),
    // and the median at or below it, or at white. STF fails too on a midtones balance past its
    // limits — 1e-5 over a black of 0 needs `MTF(0.2, 1e-5) ≈ 4e-5 < MIDTONES_MIN`, and 0.99999
    // one near 1 — where auto asinh fails on a median at or past its target, 0.99999, and reaches
    // 1e-5 with a small `β`.
    for (median, sigma, asinh_fails) in [
        (1.5f32, 0.001f32, true),
        (0.0, 0.001, true),
        (-0.001, 0.001, true),
        (1.0, 0.001, true),
        (1e-5, 1.0, false),
        (0.999_99, 1.0, true),
    ] {
        let asinh = auto_asinh_curve(median, sigma, 1.5, 0.2);
        assert_eq!(
            asinh.is_err(),
            asinh_fails,
            "asinh: median {median}, sigma {sigma}"
        );
        for (name, curve) in [
            ("STF", stf_curve(median, sigma, 1.5, 0.2)),
            ("asinh", asinh),
        ]
        .into_iter()
        .filter(|(name, _)| *name == "STF" || asinh_fails)
        {
            assert!(
                matches!(
                    curve,
                    Err(OpError::UnreachableBackground { median: reported, .. }) if reported == median
                ),
                "{name}: median {median}, sigma {sigma}"
            );
        }
    }

    // Through `apply`: the image is left as it was.
    let original = vec![-0.001f32, 0.0, 0.001, -0.002];
    let mut image = gray_image(Size2us::new(2, 2), original.clone());
    assert!(matches!(
        Stretch::auto_asinh().apply(&mut image),
        Err(OpError::UnreachableBackground { .. })
    ));
    assert_eq!(image.channel(0).pixels(), original.as_slice());
}

#[test]
fn stf_params_hand_computed() {
    // median=0.1, sigma=0.02, shadow_sigmas=1.0, target=0.25:
    //   black = 0.1 - 0.02 = 0.08
    //   rescaled median = (0.1-0.08)/(1-0.08) = 0.0217391
    //   midtones = MTF(0.25, 0.0217391) = 0.0625
    //   eval(0.1) = MTF(0.0625, 0.0217391) = 0.25   (self-inverse: median maps to target)
    //
    // Each value takes a few f32 roundings along the way; 16ε of it holds them.
    let c = stf_curve(0.1, 0.02, 1.0, 0.25).unwrap();
    let near = |got: f32, expected: f32| (got - expected).abs() <= 16.0 * f32::EPSILON * expected;
    assert_eq!(c.black.black, 0.1 - 0.02, "black");
    assert_eq!(c.black.inv_range, 1.0 / (1.0 - c.black.black), "inv_range");
    let Tone::Mtf(tone) = c.tone else {
        panic!("an STF curve is an MTF");
    };
    assert!(near(tone.midtones, 0.0625), "midtones = {}", tone.midtones);
    assert!(
        near(c.eval(0.1), 0.25),
        "the median maps to the target: {}",
        c.eval(0.1)
    );
}

/// The automatic presets are PixInsight AutoSTF's: the black point 2.8 normalized MADs below the
/// median, and the median on 0.25. A median of 0.1 and a σ of 0.02 put it at 0.1 − 0.056 = 0.044.
/// More shadow sigmas lower the black point, the same way for both auto methods.
#[test]
fn the_auto_black_point_is_autostfs() {
    assert_eq!(StretchMethod::AUTO_SHADOW_SIGMAS, 2.8);
    assert_eq!(StretchMethod::AUTO_TARGET_BACKGROUND, 0.25);
    let preset = |curve: fn(f32, f32, f32, f32) -> Result<Curve, OpError>, shadow_sigmas: f32| {
        curve(
            0.1,
            0.02,
            shadow_sigmas,
            StretchMethod::AUTO_TARGET_BACKGROUND,
        )
        .unwrap()
        .black
        .black
    };
    for curve in [stf_curve, auto_asinh_curve] {
        assert_eq!(
            preset(curve, StretchMethod::AUTO_SHADOW_SIGMAS),
            0.1 - 2.8 * 0.02
        );
        assert_eq!(preset(curve, 1.0), 0.1 - 0.02);
        assert_eq!(preset(curve, 3.0), 0.1 - 3.0 * 0.02);
    }
}

#[test]
fn ghs_endpoints_and_monotonic_across_b_family() {
    // Every b case (b=-1 log, b=0 exp, b>0 hyperbolic, general b<0) must map 0->0, 1->1, monotone.
    for &b in &[-2.0f32, -1.4, -1.0, -0.3, 0.0, 0.5, 1.0, 3.0] {
        for &d in &[0.5f32, 2.0, 6.0] {
            let c = GhsCurve::new(d, b, 0.3, 0.0, 1.0);
            assert_eq!(c.eval(0.0), 0.0, "f(0)=0 (b={b}, d={d})");
            assert!(
                (c.eval(1.0) - 1.0).abs() <= GHS_EVAL_BOUND,
                "f(1)=1 (b={b}, d={d})"
            );
            let mut prev = f32::NEG_INFINITY;
            for i in 0..=200 {
                let y = c.eval(i as f32 / 200.0);
                assert!(y >= prev, "monotonic (b={b}, d={d})");
                prev = y;
            }
        }
    }
}

/// The textbook GHS base `T(u)` in f64, per `b` branch.
fn ghs_reference_t(d: f64, b: f64, u: f64) -> f64 {
    if b == 0.0 {
        1.0 - (-d * u).exp()
    } else if b == -1.0 {
        (d * u).ln_1p()
    } else if b < 0.0 {
        (1.0 - (1.0 - b * d * u).powf((b + 1.0) / b)) / (d * (b + 1.0))
    } else {
        1.0 - (1.0 + b * d * u).powf(-1.0 / b)
    }
}

/// The textbook slope `T′(u)` of [`ghs_reference_t`].
fn ghs_reference_tp(d: f64, b: f64, u: f64) -> f64 {
    if b == 0.0 {
        d * (-d * u).exp()
    } else if b == -1.0 {
        d / (1.0 + d * u)
    } else if b < 0.0 {
        (1.0 - b * d * u).powf(1.0 / b)
    } else {
        d * (1.0 + b * d * u).powf(-(1.0 + b) / b)
    }
}

/// The textbook GHS, in f64: the base `T` and its slope per `b` branch, the mirror about `sp`, the
/// linear tails past `lp` and `hp`, normalized to [0, 1].
fn ghs_reference(d: f64, b: f64, sp: f64, lp: f64, hp: f64, x: f64) -> f64 {
    let t = |u: f64| ghs_reference_t(d, b, u);
    let tp = |u: f64| ghs_reference_tp(d, b, u);
    let raw = |x: f64| {
        if x < lp {
            tp(sp - lp) * (x - lp) - t(sp - lp)
        } else if x < sp {
            -t(sp - x)
        } else if x < hp {
            t(x - sp)
        } else {
            tp(hp - sp) * (x - hp) + t(hp - sp)
        }
    };
    ((raw(x) - raw(0.0)) / (raw(1.0) - raw(0.0))).clamp(0.0, 1.0)
}

/// The f32 curve against the f64 textbook one at every 1/400 of [0, 1], through both limits of
/// `b` — at them, and 2e-6 and 1e-5 off them, where the textbook forms cancel in f32 by up to 21%
/// — and across `d` and the protection points. The f64 forms are accurate to ~1e-10 there; the
/// f32 curve rounds each base value to a few ε and divides by a range of order one, held to 64ε.
#[test]
fn ghs_matches_an_f64_reference_through_both_limits() {
    let bound = 64.0 * f64::from(f32::EPSILON);
    for b in [
        -3.0f32,
        -1.0 - 1e-5,
        -1.0 - 2e-6,
        -1.0,
        -1.0 + 2e-6,
        -1.0 + 1e-5,
        -0.5,
        -1e-5,
        -2e-6,
        0.0,
        2e-6,
        1e-5,
        0.5,
        2.0,
    ] {
        for d in [0.5f32, 5.0, 12.0] {
            for (sp, lp, hp) in [(0.3f32, 0.0f32, 1.0f32), (0.4, 0.15, 0.85)] {
                let curve = GhsCurve::new(d, b, sp, lp, hp);
                for i in 0..=400 {
                    let x = i as f32 / 400.0;
                    let expected = ghs_reference(
                        f64::from(d),
                        f64::from(b),
                        f64::from(sp),
                        f64::from(lp),
                        f64::from(hp),
                        f64::from(x),
                    );
                    let got = f64::from(curve.eval(x));
                    assert!(
                        (got - expected).abs() <= bound,
                        "b {b}, d {d}, sp {sp}: at {x} {got} vs {expected}"
                    );
                }
            }
        }
    }
}

/// The slope the linear tails take is the base's derivative. Away from the limits of `b`, the
/// textbook slope is the central difference of the textbook base in f64 (step 1e-5: truncation
/// `h²·T‴/6` under 1e-8 of it for these `d`, rounding `ε·T/h` under 1e-10) — near them the
/// textbook forms themselves cancel, by `1/|b + 1|`, too much for a difference. And at every `b`
/// the f32 base and slope are the textbook ones, each at the scale its branch fixes — `1/d` at
/// `b = −1`, 1 elsewhere, which normalization divides out — to 16ε relative.
#[test]
fn ghs_slope_is_the_derivative_of_the_base() {
    let h = 1e-5;
    for b in [-3.0f32, -1.0 - 1e-5, -1.0, -0.5, -2e-6, 0.0, 2e-6, 0.5, 2.0] {
        for d in [0.5f32, 5.0] {
            let (b64, d64) = (f64::from(b), f64::from(d));
            let scale = if b == -1.0 { 1.0 / d64 } else { 1.0 };
            for i in 1..20 {
                let u = f64::from(i) / 20.0;
                let far_from_the_limits = [-3.0, -1.0, -0.5, 0.0, 0.5, 2.0].contains(&b);
                if far_from_the_limits {
                    let difference = (ghs_reference_t(d64, b64, u + h)
                        - ghs_reference_t(d64, b64, u - h))
                        / (2.0 * h);
                    let slope = ghs_reference_tp(d64, b64, u);
                    assert!(
                        (difference - slope).abs() <= 1e-7 * slope,
                        "b {b}, d {d}, u {u}: {difference} vs {slope}"
                    );
                }
                let near = |got: f32, expected: f64| {
                    (f64::from(got) - expected).abs()
                        <= 16.0 * f64::from(f32::EPSILON) * expected.abs()
                };
                let u32 = u as f32;
                let t = ghs_reference_t(d64, b64, f64::from(u32)) * scale;
                let tp = ghs_reference_tp(d64, b64, f64::from(u32)) * scale;
                assert!(near(ghs_base_t(d, b, u32), t), "b {b}, d {d}, u {u}: T");
                assert!(near(ghs_base_tp(d, b, u32), tp), "b {b}, d {d}, u {u}: T′");
            }
        }
    }
}

#[test]
fn ghs_identity_when_d_zero() {
    let c = GhsCurve::new(0.0, 1.0, 0.3, 0.1, 0.9);
    for &x in &[0.0f32, 0.05, 0.3, 0.5, 0.9, 1.0] {
        assert_eq!(c.eval(x), x, "d=0 is the identity at {x}");
    }
}

/// Every intermediate of `eval` lies in `[t0, T4(1)]`, the span the normalization maps to [0, 1],
/// so each of its four roundings and the base's few costs at most ε of the output: 8ε absolute.
const GHS_EVAL_BOUND: f32 = 8.0 * f32::EPSILON;

/// `b = 0`, `sp = lp = 0`, `hp = 1`, `d = 2` reduces to `f(x) = (1 − e^(−2x)) / (1 − e^(−2))`:
/// `f(0.5) = 0.632121/0.864665 = 0.731059`, `f(0.25) = 0.393469/0.864665 = 0.455054`. The f64
/// reference is that closed form, and the curve meets it to [`GHS_EVAL_BOUND`].
#[test]
fn ghs_exponential_b0_hand_computed() {
    let c = GhsCurve::new(2.0, 0.0, 0.0, 0.0, 1.0);
    for (x, hand) in [(0.5f32, 0.731_059), (0.25, 0.455_054)] {
        let closed = (1.0 - (-2.0 * f64::from(x)).exp()) / (1.0 - (-2.0f64).exp());
        assert!((closed - hand).abs() < 1e-6, "{closed} vs {hand}");
        let reference = ghs_reference(2.0, 0.0, 0.0, 0.0, 1.0, f64::from(x));
        assert!(
            (reference - closed).abs() < 1e-15,
            "{reference} vs {closed}"
        );
        assert!(
            (f64::from(c.eval(x)) - closed).abs() <= f64::from(GHS_EVAL_BOUND),
            "f({x}) = {}",
            c.eval(x)
        );
    }
}

/// The tails are straight: `f(0) = 0`, so `f(lp/2) = f(lp)/2`, and `f(1) = 1`, so
/// `f(0.9) = (f(0.8) + 1)/2` — two evaluations' [`GHS_EVAL_BOUND`] apart at most.
#[test]
fn ghs_protection_tails_are_linear() {
    let c = GhsCurve::new(3.0, 1.0, 0.5, 0.2, 0.8);
    assert_eq!(c.eval(0.0), 0.0);
    assert_eq!(c.eval(1.0), 1.0);
    assert!(
        (c.eval(0.1) - 0.5 * c.eval(0.2)).abs() <= 2.0 * GHS_EVAL_BOUND,
        "shadow tail linear from the origin"
    );
    assert!(
        (c.eval(0.9) - f32::midpoint(c.eval(0.8), 1.0)).abs() <= 2.0 * GHS_EVAL_BOUND,
        "highlight tail linear to white"
    );
}

/// No jump at `lp`, `sp` or `hp` (`b = −1.4` runs the general `b < 0` form): across one ulp of a
/// breakpoint the curve moves by its slope there times that ulp, plus each side's
/// [`GHS_EVAL_BOUND`]. The tails are tangent to the base, which
/// [`ghs_slope_is_the_derivative_of_the_base`] holds, so the curve is C¹ there too.
#[test]
fn ghs_continuous_at_breakpoints() {
    let (d, b, sp, lp, hp) = (2.5, -1.4, 0.4, 0.15, 0.85);
    let c = GhsCurve::new(d, b, sp, lp, hp);
    for bp in [lp, sp, hp] {
        let below = bp.next_down();
        let slope = ghs_base_tp(d, b, (bp - sp).abs()) * c.inv_range;
        let bound = slope * (bp - below) + 2.0 * GHS_EVAL_BOUND;
        let jump = (c.eval(bp) - c.eval(below)).abs();
        assert!(jump <= bound, "at {bp}: {jump:e} > {bound:e}");
    }
}

#[test]
fn ghs_clamps_out_of_range_input() {
    let c = GhsCurve::new(2.0, 1.0, 0.3, 0.0, 0.9);
    assert_eq!(
        c.eval(5.0),
        1.0,
        "above-1 input (a bright star) clamps to white"
    );
    assert_eq!(c.eval(-2.0), 0.0, "negative input clamps to black");
}

#[test]
fn ghs_d_controls_strength() {
    // With the symmetry point at the faint-signal level, stronger d lifts signal above sp higher
    // (below sp the antisymmetric curve instead compresses toward black).
    let weak = GhsCurve::new(1.0, 0.0, 0.1, 0.0, 1.0).eval(0.2);
    let strong = GhsCurve::new(6.0, 0.0, 0.1, 0.0, 1.0).eval(0.2);
    assert!(
        strong > weak,
        "stronger d lifts signal above sp more ({strong} > {weak})"
    );
}

#[test]
fn ghs_end_to_end_lifts_the_background() {
    let mut px: Vec<f32> = (0..90).map(|i| 0.04 + (i % 3) as f32 * 0.01).collect();
    px.extend(iter::repeat_n(0.8f32, 10));
    let mut img = gray(Size2us::new(10, 10), px.clone());
    Stretch {
        method: StretchMethod::Ghs {
            black_point: 0.0,
            d: 5.0,
            b: 0.0,
            sp: 0.1,
            lp: 0.0,
            hp: 1.0,
        },
        color: ColorMode::ColorPreserving,
    }
    .apply(&mut img)
    .unwrap();
    let out = img.channel(0).to_vec();
    assert!(median_of(&out) > median_of(&px), "background lifted");
    assert!(out[95] > out[0], "stars stay brighter than the background");
}

#[test]
fn color_preserving_keeps_channel_ratio_and_caps_highlights() {
    // Two pixels with a 2:1:1 R:G:B ratio; pixel 1 is bright enough to trip the highlight guard.
    let mut img = rgb(
        Size2us::new(2, 1),
        vec![0.3, 0.9],
        vec![0.15, 0.45],
        vec![0.15, 0.45],
    );
    let cfg = Stretch {
        method: StretchMethod::Asinh {
            black_point: 0.0,
            beta: 0.05,
        },
        color: ColorMode::ColorPreserving,
    };
    cfg.apply(&mut img).unwrap();
    let r = img.channel(0).to_vec();
    let g = img.channel(1).to_vec();
    let b = img.channel(2).to_vec();
    // 0.3/0.15 is 2 exactly in f32; the one gain rounds each channel once (2ε on the ratio), the
    // highlight cap once more (4ε), and leaves the brightest channel at 1 to its two roundings.
    let ratio = |px: usize| r[px] / g[px];
    assert!((ratio(0) - 2.0).abs() <= 4.0 * f32::EPSILON, "{}", ratio(0));
    assert_eq!(g[0], b[0]);
    assert!(r[0] < 1.0, "pixel 0 stays below white");
    assert!((ratio(1) - 2.0).abs() <= 8.0 * f32::EPSILON, "{}", ratio(1));
    assert!((r[1] - 1.0).abs() <= 2.0 * f32::EPSILON, "{}", r[1]);
}

#[test]
fn per_channel_neutralizes_color_preserving_keeps_it() {
    // Background that is redder (R≈0.20) than green/blue (≈0.05), plus one white star.
    let r = vec![0.20, 0.21, 0.19, 0.20, 0.50];
    let g = vec![0.05, 0.06, 0.04, 0.05, 0.50];
    let b = vec![0.05, 0.06, 0.04, 0.05, 0.50];
    let mut linked = rgb(Size2us::new(5, 1), r.clone(), g.clone(), b.clone());
    let mut unlinked = rgb(Size2us::new(5, 1), r, g, b);

    Stretch::auto_stf().apply(&mut linked).unwrap();
    Stretch {
        method: StretchMethod::AutoStf {
            shadow_sigmas: 1.0,
            target_background: 0.25,
        },
        color: ColorMode::PerChannel,
    }
    .apply(&mut unlinked)
    .unwrap();

    let lr = linked.channel(0).to_vec();
    let lg = linked.channel(1).to_vec();
    let ur = unlinked.channel(0).to_vec();
    let ug = unlinked.channel(1).to_vec();
    // Color-preserving keeps the red bias in the background.
    assert!(
        lr[0] > lg[0] + 0.1,
        "color-preserving keeps red > green ({}, {})",
        lr[0],
        lg[0]
    );
    // Per-channel maps each channel's own median — pixel 0 in both — onto the target: neutral
    // grey, to a few f32 roundings (16ε).
    for (channel, value) in [("red", ur[0]), ("green", ug[0])] {
        assert!(
            (value - 0.25).abs() <= 16.0 * f32::EPSILON * 0.25,
            "{channel} background {value}"
        );
    }
}

#[test]
fn end_to_end_gray_auto_stf_brightens_background_to_target() {
    // Background ~0.05 with spread (MAD > 0) plus bright stars.
    let mut px = Vec::new();
    for i in 0..90 {
        px.push(0.04 + (i % 3) as f32 * 0.01); // {0.04, 0.05, 0.06} -> median 0.05, MAD 0.01
    }
    px.extend(iter::repeat_n(0.6f32, 10));
    let input_median = median_of(&px);

    let mut img = gray(Size2us::new(10, 10), px);
    let stretch = Stretch::auto_stf();
    let StretchMethod::AutoStf {
        target_background, ..
    } = stretch.method
    else {
        unreachable!("auto_stf is an auto STF")
    };
    stretch.apply(&mut img).unwrap();
    let out = img.channel(0).to_vec();

    // The curve is built to map the median onto the target, to a few f32 roundings (16ε).
    let out_median = median_of(&out);
    assert!(out_median > input_median, "{out_median} > {input_median}");
    assert!(
        (out_median - target_background).abs() <= 16.0 * f32::EPSILON * target_background,
        "the background lands on {target_background}: {out_median}"
    );
    // Monotonic mapping: a star pixel (input 0.6) stays brighter than the background.
    assert!(out[95] > out[0], "stars stay brighter than the background");
}

/// Past [`MAX_STATISTIC_SAMPLES`] pixels the auto stretches sample with a stride: 1001 × 1000 takes
/// every second pixel. The samples read from the planes are those of the intensity plane.
#[test]
fn subsampled_intensity_is_the_intensity_plane_subsampled() {
    let size = Size2us::new(1001, 1000);
    let plane = |seed: u32| -> Vec<f32> {
        (0..size.pixel_count() as u32)
            .map(|i| (i.wrapping_mul(2_654_435_761).wrapping_add(seed) >> 8) as f32 / 16_777_216.0)
            .collect()
    };
    let image = rgb(size, plane(1), plane(2), plane(3));
    let sample = Subsample::new(size.pixel_count(), MAX_STATISTIC_SAMPLES);
    assert_eq!(sample.count(), 500_500);
    let expected: Vec<f32> = sample.of(image.intensity_plane().pixels()).collect();
    assert_eq!(subsample_intensity(&image), expected);
}

#[test]
fn default_config_is_color_preserving_auto_asinh() {
    let cfg = Stretch::default();
    assert_eq!(cfg.color, ColorMode::ColorPreserving);
    assert!(matches!(cfg.method, StretchMethod::AutoAsinh { .. }));
}

/// Faint Hα, (0.055, 0.05, 0.05) on a sky of 0.05, keeps its hue: the ratio is formed after the
/// black point, so with the black point on the sky what is left is pure red — green and blue come
/// out 0 exactly. Formed on the data with the sky in it, the same pixel comes out at the ratio
/// 0.055 : 0.05, near grey.
#[test]
fn faint_h_alpha_keeps_its_hue() {
    let stretch = |black_point: f32| {
        let mut image = rgb(Size2us::new(1, 1), vec![0.055], vec![0.05], vec![0.05]);
        Stretch {
            method: StretchMethod::Asinh {
                black_point,
                beta: 0.01,
            },
            color: ColorMode::ColorPreserving,
        }
        .apply(&mut image)
        .unwrap();
        [0, 1, 2].map(|channel| image.channel(channel).pixels()[0])
    };
    let [r, g, b] = stretch(0.05);
    assert!(r > 0.0, "{r}");
    assert_eq!([g, b], [0.0, 0.0]);
    let [r, g, _] = stretch(0.0);
    assert!((r / g - 1.1).abs() < 1e-5, "the control: {}", r / g);
}

/// The automatic black point sits below the sky, so a faint red pixel keeps its excess over it:
/// the output channels stand as `c − black`. The sky runs 0.049, 0.05, 0.051 under a pixel of
/// (0.055, 0.05, 0.05), which gives a median of 0.05 and a MAD of 0.001; the black point is the
/// production statistics' median less 2.8 normalized MADs of the intensities, and the output ratio
/// meets `(0.055 − black) / (0.05 − black)` to the roundings of the rescale (2ε per channel) and
/// the one gain (ε each): 6ε.
#[test]
fn the_auto_black_point_keeps_a_faint_colour() {
    let size = Size2us::new(16, 16);
    let sky: Vec<f32> = (0..size.pixel_count())
        .map(|index| [0.049, 0.05, 0.051][index % 3])
        .collect();
    let (mut r, g, b) = (sky.clone(), sky.clone(), sky);
    r[1] = 0.055;
    let (g0, b0) = (g[1], b[1]);
    let mut intensities: Vec<f32> = (0..size.pixel_count())
        .map(|index| (r[index] + g[index] + b[index]) * (1.0 / 3.0))
        .collect();
    let background = MedianMad::of_mut(&mut intensities);
    let black = background.median - StretchMethod::AUTO_SHADOW_SIGMAS * background.sigma();
    let mut image = rgb(size, r, g, b);
    Stretch::auto_asinh().apply(&mut image).unwrap();
    let (out_r, out_g, out_b) = (
        image.channel(0).pixels()[1],
        image.channel(1).pixels()[1],
        image.channel(2).pixels()[1],
    );
    assert_eq!(g0, b0);
    assert_eq!(out_g, out_b);
    let expected = (0.055 - black) / (g0 - black);
    assert!(
        (out_r / out_g - expected).abs() <= 6.0 * f32::EPSILON * expected,
        "{} vs {expected}",
        out_r / out_g
    );
    assert!(expected > 1.5, "the excess is a real colour: {expected}");
}

/// A NaN sample is black on every scalar curve, as on the vector paths.
#[test]
fn a_nan_sample_is_black_on_every_curve() {
    assert_eq!(MtfCurve { midtones: 0.2 }.eval(f32::NAN), 0.0);
    assert_eq!(AsinhCurve::new(0.05).eval(f32::NAN), 0.0);
    assert_eq!(GhsCurve::new(3.0, 1.0, 0.5, 0.2, 0.8).eval(f32::NAN), 0.0);
    assert_eq!(GhsCurve::new(0.0, 1.0, 0.5, 0.2, 0.8).eval(f32::NAN), 0.0);
    let mut image = rgb(Size2us::new(1, 1), vec![f32::NAN], vec![0.2], vec![0.2]);
    Stretch {
        method: StretchMethod::Ghs {
            black_point: 0.0,
            d: 3.0,
            b: 1.0,
            sp: 0.5,
            lp: 0.2,
            hp: 0.8,
        },
        color: ColorMode::ColorPreserving,
    }
    .apply(&mut image)
    .unwrap();
    for channel in 0..3 {
        assert_eq!(image.channel(channel).pixels(), &[0.0], "channel {channel}");
    }
}
