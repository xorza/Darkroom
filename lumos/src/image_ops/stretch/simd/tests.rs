//! Every tier against the scalar curve.

use crate::image_ops::stretch::simd::{AsinhColorPreserve, AsinhPlane};
use crate::image_ops::stretch::{AsinhCurve, BlackPoint};
use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
use simd::IsaTier;

/// Every tier over three planes drawn from each shape, after a black point of 0.02: the shapes put
/// intensities below it (black on both paths), inside the curve, and above the highlight cap.
#[test]
fn asinh_color_preserve_matches_scalar() {
    let curve = AsinhCurve::new(0.05);
    let black = BlackPoint::new(0.02);
    assert_simd_matches_scalar(SWEEP_WIDTHS, |tier, shape, width| {
        let mut scalar = [
            shape.row(width, 0),
            shape.row(width, 1),
            shape.row(width, 2),
        ];
        let mut simd = scalar.clone();
        let [red, green, blue] = &mut scalar;
        AsinhColorPreserve {
            red,
            green,
            blue,
            black,
            curve,
        }
        .run_scalar();
        let [red, green, blue] = &mut simd;
        tier.run(AsinhColorPreserve {
            red,
            green,
            blue,
            black,
            curve,
        });
        ScalarSimd::new(scalar.concat(), simd.concat())
    });
}

/// Every tier's plane curve against the scalar curve, after a black point of 0.02, at a curve whose
/// `1/β` puts the shapes' samples across the whole `asinh` range.
#[test]
fn asinh_plane_matches_scalar() {
    let curve = AsinhCurve::new(0.05);
    let black = BlackPoint::new(0.02);
    assert_simd_matches_scalar(SWEEP_WIDTHS, |tier, shape, width| {
        let mut scalar = shape.row(width, 0);
        let mut simd = scalar.clone();
        AsinhPlane {
            plane: &mut scalar,
            black,
            curve,
        }
        .run_scalar();
        tier.run(AsinhPlane {
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
#[test]
fn a_nan_sample_is_black_on_every_path() {
    let curve = AsinhCurve::new(0.05);
    let black = BlackPoint::new(0.02);
    let mut scalar = [f32::NAN, 0.5];
    AsinhPlane {
        plane: &mut scalar,
        black,
        curve,
    }
    .run_scalar();
    assert_eq!(scalar[0], 0.0);
    let mut red = [f32::NAN, 0.3];
    let mut green = [0.2, f32::NAN];
    let mut blue = [0.2, 0.2];
    AsinhColorPreserve {
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
