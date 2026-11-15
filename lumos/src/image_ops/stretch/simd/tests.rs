//! Every tier against the scalar curve.

use crate::image_ops::stretch::simd::{ToneColorPreserve, TonePlane, VectorTone};
use crate::image_ops::stretch::{AsinhCurve, BlackPoint, GhsCurve, GhsShape, ToneCurve};
use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
use simd::IsaTier;

/// The black point every case runs after: the shapes put samples below it.
const BLACK: f32 = 0.02;

/// A GHS of every form `b` selects — the logarithmic limit, the hyperbolic `b < 0`, the
/// exponential limit and `b > 0` — with the symmetry point and both protection points inside the
/// shapes' range, so every case meets the shadow tail, the mirrored base, the base and the
/// highlight tail.
fn ghs_curves() -> impl Iterator<Item = GhsCurve> {
    [-3.0, -1.0, -0.5, 0.0, 0.5, 2.0]
        .map(|b| {
            GhsCurve::new(GhsShape {
                d: 5.0,
                b,
                sp: 0.1,
                lp: 0.02,
                hp: 0.9,
            })
            .expect("a GHS of strength 5")
        })
        .into_iter()
}

/// Every tier over three planes drawn from each shape: the shapes put intensities below the black
/// point (black on both paths), inside the curve, and above the highlight cap.
fn color_preserve_matches_scalar<T: VectorTone + ToneCurve>(curve: T) {
    let black = BlackPoint::new(BLACK);
    assert_simd_matches_scalar(SWEEP_WIDTHS, |tier, shape, width| {
        let mut scalar = [
            shape.row(width, 0),
            shape.row(width, 1),
            shape.row(width, 2),
        ];
        let mut simd = scalar.clone();
        let [red, green, blue] = &mut scalar;
        ToneColorPreserve {
            red,
            green,
            blue,
            black,
            curve,
        }
        .run_scalar();
        let [red, green, blue] = &mut simd;
        tier.run(ToneColorPreserve {
            red,
            green,
            blue,
            black,
            curve,
        });
        ScalarSimd::new(scalar.concat(), simd.concat())
    });
}

/// Every tier's plane curve against the scalar curve, over each shape.
fn plane_matches_scalar<T: VectorTone + ToneCurve>(curve: T) {
    let black = BlackPoint::new(BLACK);
    assert_simd_matches_scalar(SWEEP_WIDTHS, |tier, shape, width| {
        let mut scalar = shape.row(width, 0);
        let mut simd = scalar.clone();
        TonePlane {
            plane: &mut scalar,
            black,
            curve,
        }
        .run_scalar();
        tier.run(TonePlane {
            plane: &mut simd,
            black,
            curve,
        });
        ScalarSimd::new(scalar, simd)
    });
}

/// A NaN sample shows as black on every tier and on the scalar path: on the plane, and in any
/// channel of a color pixel, whose other channels go black with it because their intensity is
/// NaN.
fn a_nan_sample_is_black<T: VectorTone + ToneCurve>(curve: T) {
    let black = BlackPoint::new(BLACK);
    let mut scalar = [f32::NAN, 0.5];
    TonePlane {
        plane: &mut scalar,
        black,
        curve,
    }
    .run_scalar();
    assert_eq!(scalar[0], 0.0);
    let mut red = [f32::NAN, 0.3];
    let mut green = [0.2, f32::NAN];
    let mut blue = [0.2, 0.2];
    ToneColorPreserve {
        red: &mut red,
        green: &mut green,
        blue: &mut blue,
        black,
        curve,
    }
    .run_scalar();
    assert_eq!([red, green, blue], [[0.0; 2]; 3]);
    for tier in IsaTier::supported() {
        let mut plane = [f32::NAN, 0.5];
        tier.run(TonePlane {
            plane: &mut plane,
            black,
            curve,
        });
        assert_eq!(plane[0], 0.0, "{tier}");
        let (mut red, mut green, mut blue) = ([f32::NAN, 0.3], [0.2, f32::NAN], [0.2, 0.2]);
        tier.run(ToneColorPreserve {
            red: &mut red,
            green: &mut green,
            blue: &mut blue,
            black,
            curve,
        });
        assert_eq!([red, green, blue], [[0.0; 2]; 3], "{tier}");
    }
}

/// The arcsinh at a `1/β` that puts the shapes' samples across the whole `asinh` range.
#[test]
fn asinh_kernels_match_scalar() {
    let curve = AsinhCurve::new(0.05);
    plane_matches_scalar(curve);
    color_preserve_matches_scalar(curve);
    a_nan_sample_is_black(curve);
}

#[test]
fn ghs_kernels_match_scalar() {
    for curve in ghs_curves() {
        plane_matches_scalar(curve);
        color_preserve_matches_scalar(curve);
        a_nan_sample_is_black(curve);
    }
}
