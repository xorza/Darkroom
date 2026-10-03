use super::*;
use crate::math::size2us::Size2us;
use std::f64::consts::PI;

#[test]
fn insufficient_point_count_scales_with_order() {
    // Verify the 3x multiplier: each order needs 3 * term_count points minimum.
    // Order 2: 3 terms -> 9 min
    // Order 3: 7 terms -> 21 min
    // Order 4: 12 terms -> 36 min
    // Order 5: 18 terms -> 54 min
    let transform = Transform::identity();

    for (order, expected_terms) in [(2, 3), (3, 7), (4, 12), (5, 18)] {
        let min_needed = 3 * expected_terms;
        let config = SipConfig {
            order,
            reference_point: Some(DVec2::ZERO),
            ..Default::default()
        };

        let ref_pts: Vec<DVec2> = (0..min_needed - 1)
            .map(|i| {
                let row = i / 10;
                let col = i % 10;
                DVec2::new(col as f64 * 100.0, row as f64 * 100.0)
            })
            .collect();
        let tgt_pts = ref_pts.clone();

        let error =
            SipPolynomial::fit_from_transform(&ref_pts, &tgt_pts, &transform, &config).unwrap_err();
        match error {
            RegistrationError::InsufficientSipPoints { found, required } => {
                assert_eq!(found, min_needed - 1);
                assert_eq!(required, min_needed);
            }
            other => panic!("expected insufficient SIP points error, got {other:?}"),
        }
    }
}

#[test]
fn invalid_config_returns_error() {
    let ref_points = vec![DVec2::ZERO; 10];
    let target_points = vec![DVec2::ZERO; 10];
    let transform = Transform::identity();

    for (config, expected_field) in [
        (
            SipConfig {
                order: 1,
                ..Default::default()
            },
            "SIP order",
        ),
        (
            SipConfig {
                order: 6,
                ..Default::default()
            },
            "SIP order",
        ),
        (
            SipConfig {
                clip_sigma: 0.0,
                ..Default::default()
            },
            "SIP clip_sigma",
        ),
    ] {
        let error =
            SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
                .unwrap_err();

        match error {
            RegistrationError::InvalidConfig(invalid) => {
                assert_eq!(invalid.field, expected_field);
            }
            other => panic!("expected invalid configuration error, got {other:?}"),
        }
    }
}

#[test]
fn mismatched_and_singular_fits_return_exact_errors() {
    let config = SipConfig::default();
    let ref_points = vec![DVec2::ZERO; 30];
    let target_points = vec![DVec2::ZERO; 20];
    let transform = Transform::identity();
    let mismatch =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
            .unwrap_err();
    assert!(matches!(
        mismatch,
        RegistrationError::SipPointCountMismatch {
            reference: 30,
            target: 20
        }
    ));

    let singular_config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::ZERO),
        ..Default::default()
    };
    let coincident_points = vec![DVec2::ZERO; 9];
    let singular = SipPolynomial::fit_from_transform(
        &coincident_points,
        &coincident_points,
        &transform,
        &singular_config,
    )
    .unwrap_err();
    assert!(matches!(singular, RegistrationError::SingularSipSystem));
}

/// `max_correction` is the largest correction over a grid on the frame, corners included: the
/// barrel field's farthest point from the centre is a corner at `d = (−500, −500)`, where `|d|² =
/// 500 000` and the correction `d·k·|d|²` is `(−25, −25)`, of size `25√2`. An undistorted field has
/// none.
#[test]
fn max_correction_is_the_corner_correction() {
    let size = Size2us::new(1000, 1000);
    let barrel = fit_field(&barrel(), 3, 3).polynomial;
    let corner = 25.0 * 2f64.sqrt();
    assert!((barrel.max_grid_correction(size, 50.0) - corner).abs() <= EXACT_FIT_PX);
    let flat = fit_field(
        &RadialField {
            k: 0.0,
            ..super::barrel()
        },
        2,
        3,
    )
    .polynomial;
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
/// Up to affine, the fit target `J⁻¹·(t − T(r))` is `d` exactly and order 3 holds a cubic exactly,
/// so what remains is rounding: coordinates of order 10³ resolved to `u·10³` ≈ 1e-13, amplified by
/// the conditioning of the normalized design, far under 1e-8 px. For the homography the target is
/// first order: the second-order term `½·|∂²T|·|d|²`, with `|∂²T|` ≈ `2·|g|·|J|` ≈ 4e-6 per pixel,
/// is at most 2.5e-3 px, and 1e-2 px holds it. The warp applies the same model, so it lands on the
/// targets to the same bound.
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
            step: 50,
            ..barrel()
        };
        let RadialPairs { reference, target } = field.pairs();
        // No clipping: the field is noiseless, and the homography's second-order residual is
        // structure, not outliers, so clipping would only trim the corners it is largest at.
        let fit = fit_field(&field, 3, 0);
        assert!(
            fit.max_residual < bound,
            "{name}: max residual {:e}",
            fit.max_residual
        );

        let warp = WarpTransform::with_sip(transform, fit.polynomial);
        let worst = reference
            .iter()
            .zip(&target)
            .map(|(&r, &t)| (warp.apply(r) - t).length())
            .fold(0.0, f64::max);
        assert!(worst < bound, "{name}: warp lands {worst:e} px off");
    }
}
