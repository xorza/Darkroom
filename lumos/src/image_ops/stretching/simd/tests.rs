//! Every tier against the scalar curve.

use crate::image_ops::stretching::simd::internals::{
    asinh_color_preserve_scalar, asinh_plane_scalar,
};
use crate::image_ops::stretching::simd::{AsinhColorPreserve, AsinhPlane};
use crate::image_ops::stretching::{AsinhCurve, BlackPoint};
use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
use crate::simd::math::ASINH_LOG_FROM;
use crate::simd::tier::Tier;
use std::f32::consts::LN_2;

/// Every tier over three planes drawn from each shape, after a black point of 0.02: the shapes put
/// intensities below it (black on both paths), inside the curve, and above the highlight cap.
///
/// The kernel evaluates `asinh` through a Cephes `logf` (about 2 ULP) where the scalar path calls
/// libm's `asinhf` (at most 1 ULP), so the curve values differ by up to 3 ULP, which is 3ε
/// relative at most; the scale `f(I)/I` and the cap round once more on each side. The outputs
/// therefore agree to 4ε relative.
#[test]
fn asinh_color_preserve_matches_scalar() {
    let curve = AsinhCurve::new(0.05);
    let black = BlackPoint::new(0.02);
    assert_simd_matches_scalar(SWEEP_WIDTHS, 4.0 * f32::EPSILON, |tier, shape, width| {
        let mut scalar = [
            shape.row(width, 0),
            shape.row(width, 1),
            shape.row(width, 2),
        ];
        let mut simd = scalar.clone();
        let [r, g, b] = &mut scalar;
        asinh_color_preserve_scalar(r, g, b, black, curve);
        let [red, green, blue] = &mut simd;
        tier.run(AsinhColorPreserve {
            red,
            green,
            blue,
            black,
            curve,
        });
        ScalarSimd::relative(scalar.concat(), simd.concat())
    });
}

/// `asinh(x)` for `x ≥ 0` by the steps the kernels take, one lane at a time with libm's
/// `ln` for the Cephes `logf`: below [`ASINH_LOG_FROM`], `log1p(u)` with `u = x + x²/(1 + √(1 +
/// x²))` — `√(1 + x²) + x − 1` without its cancellation — and `log1p(u) = ln(1 + u) · u / ((1 + u)
/// − 1)`, whose ratio cancels the rounding of `1 + u` (Goldberg 1991); past it, `ln x + ln 2`. The
/// textbook `ln(x + √(x² + 1))` instead loses all relative accuracy as `x → 0`, where `x + √(x²+1)`
/// rounds to 1.
fn asinh_pos_scalar(x: f32) -> f32 {
    if x > ASINH_LOG_FROM {
        return x.ln() + LN_2;
    }
    let s = x * x;
    let u = x + s / (1.0 + (1.0 + s).sqrt());
    let w = 1.0 + u;
    let dw = w - 1.0;
    if dw == 0.0 { u } else { w.ln() * (u / dw) }
}

/// The magnitudes the curve's `asinh` sees: 0, and every power of ten from 1e-30 to 1e30 with a
/// spread of mantissas, across [`ASINH_LOG_FROM`].
fn magnitudes() -> Vec<f32> {
    let mut values = vec![0.0f32];
    for exponent in -30..=30 {
        for mantissa in [1.0f32, 1.7, 3.3, 7.9] {
            values.push(mantissa * 10f32.powi(exponent));
        }
    }
    values
}

/// The relative error of `got` against `asinh(x)` in f64.
fn relative_error(x: f32, got: f32) -> f64 {
    let truth = f64::from(x).asinh();
    if truth == 0.0 {
        f64::from(got).abs()
    } else {
        (f64::from(got) - truth).abs() / truth
    }
}

/// The steps the kernels take hold `asinh` to a few ε relative at every magnitude — through
/// `x → 0`, where the textbook `ln(x + √(x² + 1))` loses its relative accuracy as `ε/x` (6e-4 off
/// at x = 1e-4), and past [`ASINH_LOG_FROM`] — and so does every tier, whose Cephes `logf` adds
/// its ~2 ULP. The steps round `u` three times and `ln(1 + u) · u/((1 + u) − 1)` three more: 6ε for
/// the scalar steps, 8ε for the vector ones.
#[test]
fn asinh_is_accurate_at_every_magnitude() {
    let values = magnitudes();
    for &x in &values {
        let error = relative_error(x, asinh_pos_scalar(x));
        assert!(error <= 6.0 * f64::from(f32::EPSILON), "x = {x}: {error:e}");
    }
    let curve = AsinhCurve {
        inv_beta: 1.0,
        inv_norm: 1.0,
    };
    for tier in Tier::supported() {
        let mut plane = values.clone();
        tier.run(AsinhPlane {
            plane: &mut plane,
            black: BlackPoint::new(0.0),
            curve,
        });
        for (&x, &got) in values.iter().zip(&plane) {
            // The plane curve clamps to [0, 1]: compare below that.
            let expected = f64::from(x).asinh();
            if expected < 1.0 {
                let error = relative_error(x, got);
                assert!(
                    error <= 8.0 * f64::from(f32::EPSILON),
                    "{tier} x = {x}: {error:e}"
                );
            } else {
                assert_eq!(got, 1.0, "{tier} x = {x}");
            }
        }
    }
}

/// Every tier's plane curve against the scalar curve (libm's `asinhf`), after a black point of
/// 0.02, at a curve whose `1/β` puts the shapes' samples across the whole `asinh` range: both
/// within the 8ε the vector steps keep, plus the scale's and the clamp's one rounding each. The
/// black point rounds the same two operations on both paths.
#[test]
fn asinh_plane_matches_scalar() {
    let curve = AsinhCurve::new(0.05);
    let black = BlackPoint::new(0.02);
    assert_simd_matches_scalar(SWEEP_WIDTHS, 10.0 * f32::EPSILON, |tier, shape, width| {
        let mut scalar = shape.row(width, 0);
        let mut simd = scalar.clone();
        asinh_plane_scalar(&mut scalar, black, curve);
        tier.run(AsinhPlane {
            plane: &mut simd,
            black,
            curve,
        });
        ScalarSimd::relative(scalar, simd)
    });
}

/// A NaN sample shows as black on every tier and on the scalar path: on the plane, and in any
/// channel of a colour pixel, whose other channels go black with it because their intensity is
/// NaN.
#[test]
fn a_nan_sample_is_black_on_every_path() {
    let curve = AsinhCurve::new(0.05);
    let black = BlackPoint::new(0.02);
    let mut scalar = [f32::NAN, 0.5];
    asinh_plane_scalar(&mut scalar, black, curve);
    assert_eq!(scalar[0], 0.0);
    let mut red = [f32::NAN, 0.3];
    let mut green = [0.2, f32::NAN];
    let mut blue = [0.2, 0.2];
    asinh_color_preserve_scalar(&mut red, &mut green, &mut blue, black, curve);
    assert_eq!([red, green, blue], [[0.0; 2]; 3]);
    for tier in Tier::supported() {
        let mut plane = [f32::NAN, 0.5];
        tier.run(AsinhPlane {
            plane: &mut plane,
            black,
            curve,
        });
        assert_eq!(plane[0], 0.0, "{tier}");
        let (mut red, mut green, mut blue) = ([f32::NAN, 0.3], [0.2, f32::NAN], [0.2, 0.2]);
        tier.run(AsinhColorPreserve {
            red: &mut red,
            green: &mut green,
            blue: &mut blue,
            black,
            curve,
        });
        assert_eq!([red, green, blue], [[0.0; 2]; 3], "{tier}");
    }
}
