use super::*;

/// The 50 px grid over `[0, 1000]²` that the fits of a field and a linear part sample.
const STEP_50: Grid = Grid {
    step: 50,
    ..Grid::DEFAULT
};
use crate::math::size2us::Size2us;
use crate::registration::estimate::Pairs;
use crate::registration::transform::TransformType;
use std::f64::consts::PI;

/// A fit needs three pairs per term against overfitting: orders 2 to 5 have 3, 7, 12 and 18 terms,
/// so 9, 21, 36 and 54 pairs.
#[test]
fn required_points_scale_with_order() {
    for (order, terms) in [
        (SipOrder::Two, 3),
        (SipOrder::Three, 7),
        (SipOrder::Four, 12),
        (SipOrder::Five, 18),
    ] {
        assert_eq!(order.required_points(), 3 * terms, "{order:?}");
    }
}

/// The origin must be finite; each refusal names its field.
#[test]
fn invalid_config_returns_error() {
    for (reference_point, field) in [
        (DVec2::new(f64::NAN, 0.0), "SIP reference_point x"),
        (DVec2::new(0.0, f64::INFINITY), "SIP reference_point y"),
    ] {
        let config = SipConfig {
            reference_point: Some(reference_point),
            ..Default::default()
        };
        let invalid: InvalidConfigField = config.validate().unwrap_err();
        assert_eq!(invalid.field, field);
    }
    assert!(SipConfig::default().validate().is_ok());
}

/// Coincident points determine no polynomial: both fits refuse them.
#[test]
fn coincident_points_fit_nothing() {
    let coincident = vec![DVec2::ZERO; 9];
    let weights = vec![1.0; 9];
    let pairs = SipPairs {
        reference: &coincident,
        target: &coincident,
        weights: &weights,
    };
    assert!(
        SipPolynomial::fit_under(
            &Transform::identity(),
            pairs,
            SipModel {
                order: SipOrder::Two,
                origin: DVec2::ZERO
            }
        )
        .is_none()
    );
    assert!(
        SipPolynomial::fit_with(
            LinearModel::Affine,
            pairs,
            SipModel {
                order: SipOrder::Two,
                origin: DVec2::ZERO
            }
        )
        .is_none()
    );
}

/// The largest correction over a grid on the frame, corners included: the barrel field's farthest
/// point from the centre is a corner at `d = (−500, −500)`, where `|d|² = 500 000` and the
/// correction `d·k·|d|²` is `(−25, −25)`, of size `25√2`. An undistorted field has none, and every
/// coefficient of its fit is zero.
#[test]
fn max_correction_is_the_corner_correction() {
    let size = Size2us::new(1000, 1000);
    let distorted = fit_field(&barrel(), Grid::DEFAULT, SipOrder::Three);
    let corner = 25.0 * 2f64.sqrt();
    assert!((distorted.max_grid_correction(size, 50.0) - corner).abs() <= EXACT_FIT_PX);
    let flat = fit_field(
        &RadialField { k: 0.0, ..barrel() },
        Grid::DEFAULT,
        SipOrder::Two,
    );
    assert_eq!(flat.max_grid_correction(size, 50.0), 0.0);
    assert!(
        flat.coeffs_u
            .iter()
            .chain(&flat.coeffs_v)
            .all(|&c| c == 0.0)
    );
}

/// SIP corrects reference pixels before the transform, whatever the transform's linear part. The
/// field is a radial distortion in the reference frame, `d(r) = (r − c)·k·|r − c|²` with
/// `|d| ≤ 35 px`, carried through `T`: `t = T(r + d(r))`. A fit that took `t − T(r)` as the
/// correction would leave `|(J − I)·d|` — 6 px at 10°, 70 px at 180°.
///
/// Up to affine, `T(r + c) = T(r) + J·c` and order 3 holds a cubic exactly, so what remains is
/// rounding: coordinates of order 10³ resolved to `u·10³` ≈ 1e-13, amplified by the conditioning
/// of the normalized design, far under 1e-8 px. For the homography the fit is first order: the
/// second-order term `½·|∂²T|·|d|²`, with `|∂²T|` ≈ `2·|g|·|J|` ≈ 4e-6 per pixel, is at most
/// 2.5e-3 px, and 1e-2 px holds it.
#[test]
fn sip_corrects_in_the_reference_frame_under_any_linear_part() {
    let cases = [
        (
            "10°",
            Transform::similarity(DVec2::new(30.0, -20.0), 10f64.to_radians(), 1.02),
            EXACT_FIT_PX,
        ),
        (
            "180°",
            Transform::euclidean(DVec2::new(1000.0, 1000.0), PI),
            EXACT_FIT_PX,
        ),
        (
            "homography",
            Transform::homography([1.01, 0.02, 15.0, -0.015, 0.99, -8.0, 2e-6, -1e-6]),
            1e-2,
        ),
    ];
    for (name, transform, bound) in cases {
        let field = RadialField {
            transform,
            ..barrel()
        };
        let RadialPairs { reference, target } = field.pairs(STEP_50);
        let warp = WarpTransform::with_sip(transform, fit_field(&field, STEP_50, SipOrder::Three));
        let worst = reference
            .iter()
            .zip(&target)
            .map(|(&r, &t)| (warp.apply(r) - t).length())
            .fold(0.0, f64::max);
        assert!(worst < bound, "{name}: warp lands {worst:e} px off");
    }
}

/// Every linear part and its correction fitted together reach the joint optimum: a barrel field
/// before a translation, a 10° rotation, a similarity and a sheared affine map, `t = T(r + d(r))`,
/// comes back exactly — `T` and `d` to rounding — at uneven weights, which move nothing on exact
/// pairs. Fitting the affine map first leaves part of the field in it, 1 px and more off the truth.
/// The rotation's angle is the secant iteration's, to the rounding of the angle.
#[test]
fn each_linear_part_and_its_correction_fit_together() {
    let cases = [
        (
            LinearModel::Translation,
            Transform::translation(DVec2::new(12.0, -7.0)),
        ),
        (
            LinearModel::Euclidean,
            Transform::euclidean(DVec2::new(30.0, -20.0), 10f64.to_radians()),
        ),
        (
            LinearModel::Similarity,
            Transform::similarity(DVec2::new(-5.0, 9.0), -0.3, 1.02),
        ),
        (
            LinearModel::Affine,
            Transform::affine([1.01, 0.03, 12.0, -0.02, 0.98, -7.0]),
        ),
    ];
    for (model, truth) in cases {
        let field = RadialField {
            transform: truth,
            ..barrel()
        };
        let RadialPairs { reference, target } = field.pairs(STEP_50);
        let weights: Vec<f64> = (0..reference.len()).map(|i| 1.0 + (i % 4) as f64).collect();
        let pairs = SipPairs {
            reference: &reference,
            target: &target,
            weights: &weights,
        };
        let SipFit { transform, sip } = SipPolynomial::fit_with(
            model,
            pairs,
            SipModel {
                order: SipOrder::Three,
                origin: field.centre,
            },
        )
        .unwrap();
        for p in [DVec2::new(700.0, 300.0), DVec2::ZERO, DVec2::splat(1000.0)] {
            assert!(
                transform.apply(p).distance(truth.apply(p)) <= 1e-9,
                "{model:?} transform at {p:?}"
            );
            let miss = (sip.correct(p) - p - field.displacement(p)).length();
            assert!(
                miss <= EXACT_FIT_PX,
                "{model:?} correction at {p:?}: {miss:e}"
            );
        }
    }

    let RadialPairs { reference, target } = RadialField {
        transform: cases[3].1,
        ..barrel()
    }
    .pairs(STEP_50);
    let first = Pairs::unit(&reference, &target)
        .estimate(TransformType::Affine)
        .unwrap();
    let off = (first.apply(DVec2::ZERO) - cases[3].1.apply(DVec2::ZERO)).length();
    assert!(
        off > 1.0,
        "the affine map alone lands {off} px from the truth"
    );
}
