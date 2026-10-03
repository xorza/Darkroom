use super::*;

/// Without a reference point the fit centres on the points' centroid: on the grid
/// `200, 300, …, 800` that is (500, 500), the barrel's own centre, so the fit is exact.
#[test]
fn reference_point_none_uses_centroid() {
    let field = RadialField {
        start: 200,
        extent: 800,
        ..barrel()
    };
    let RadialPairs { reference, target } = field.pairs();
    let config = SipConfig {
        order: 3,
        reference_point: None,
        ..Default::default()
    };
    let sip = fit_sip(&reference, &target, &Transform::identity(), &config).polynomial;
    assert_eq!(
        sip.norm,
        PointNormalization::around(&reference, field.centre)
    );
    let residuals = sip.corrected_residuals(&reference, &target, &Transform::identity());
    assert!(rms(&residuals) <= EXACT_FIT_PX);
}

/// SIP has no constant or linear terms — those belong to the transform — so a radial field is a
/// SIP polynomial only about its own centre. Points in one quadrant of a field centred at
/// (512, 384): fitted about that centre, exact; about the points' centroid, the field's shift of
/// origin leaves linear terms the polynomial cannot hold.
#[test]
fn a_radial_field_is_exact_only_about_its_centre() {
    let field = RadialField::new(DVec2::new(512.0, 384.0), 1e-7);
    let mut reference = Vec::new();
    for y in (100..=350).step_by(50) {
        for x in (100..=450).step_by(50) {
            reference.push(DVec2::new(f64::from(x), f64::from(y)));
        }
    }
    let target: Vec<DVec2> = reference.iter().map(|&p| field.image(p)).collect();
    let rms_about = |reference_point| {
        let config = SipConfig {
            order: 3,
            reference_point,
            ..Default::default()
        };
        let sip = fit_sip(&reference, &target, &Transform::identity(), &config).polynomial;
        rms(&sip.corrected_residuals(&reference, &target, &Transform::identity()))
    };
    let centred = rms_about(Some(field.centre));
    let centroid = rms_about(None);
    assert!(centred <= EXACT_FIT_PX, "{centred:e}");
    assert!(centroid > 1e3 * EXACT_FIT_PX, "{centroid:e}");
}

/// Sigma clipping drops gross outliers and refits on the rest: three points 20–30 px off the barrel
/// field, among 121 on it. Clipped, the fit of the clean points is exact; unclipped, the outliers
/// pull it off. On clean data clipping finds nothing to drop, and the fit is the unclipped one bit
/// for bit.
#[test]
fn sigma_clipping_drops_outliers_and_leaves_clean_data_alone() {
    let RadialPairs {
        mut reference,
        mut target,
    } = barrel().pairs();
    let clean = reference.len();
    for (r, t) in [
        ([300.0, 300.0], [320.0, 280.0]),
        ([700.0, 200.0], [685.0, 225.0]),
        ([100.0, 800.0], [130.0, 810.0]),
    ] {
        reference.push(DVec2::from_array(r));
        target.push(DVec2::from_array(t));
    }
    let clean_rms = |clip_iterations| {
        let config = SipConfig {
            order: 3,
            reference_point: Some(barrel().centre),
            clip_iterations,
            ..Default::default()
        };
        let sip = fit_sip(&reference, &target, &Transform::identity(), &config).polynomial;
        rms(&sip.corrected_residuals(
            &reference[..clean],
            &target[..clean],
            &Transform::identity(),
        ))
    };
    assert!(clean_rms(3) <= EXACT_FIT_PX, "{:e}", clean_rms(3));
    assert!(clean_rms(0) > 1e3 * EXACT_FIT_PX, "{:e}", clean_rms(0));

    let clipped = fit_field(&barrel(), 3, 3).polynomial;
    let unclipped = fit_field(&barrel(), 3, 0).polynomial;
    assert_eq!(clipped.coeffs_u, unclipped.coeffs_u);
    assert_eq!(clipped.coeffs_v, unclipped.coeffs_v);
}

/// A narrow strip — x over 1000 px, y over 100 — makes the v-dependent monomials tiny beside the
/// u-dependent ones, and its normal equations would square that conditioning. The SVD solve on the
/// design matrix keeps it, so the exact cubic field is recovered to rounding inside the strip:
/// coordinates of order 10³ resolved to `u·10³` ≈ 1e-13, under 1e-9 px with the design's
/// conditioning on top.
#[test]
fn a_narrow_strip_is_fitted_to_rounding() {
    let field = barrel();
    let center = field.centre;
    let mut ref_points = Vec::new();
    for y in (450..=550).step_by(10) {
        for x in (0..=1000).step_by(20) {
            ref_points.push(DVec2::new(f64::from(x), f64::from(y)));
        }
    }
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| field.image(p)).collect();
    // 11 y-values × 51 x-values = 561 points; order 5 needs 3 × 18 = 54.

    let transform = Transform::identity();
    let config = SipConfig {
        order: 5,
        reference_point: Some(center),
        ..Default::default()
    };
    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    for x_val in (0..=1000).step_by(100) {
        let p = DVec2::new(f64::from(x_val), 500.0);
        let error = (transform.apply(sip.correct(p)) - field.image(p)).length();
        assert!(
            error <= EXACT_FIT_PX,
            "strip fit error at x={x_val}: {error:e} px"
        );
    }
}

/// Clipping never refits on fewer points than the fit itself demands. Order 2 needs 3 × 3 = 9
/// points; nine points with one gross outlier would clip to eight, so the first fit stands, with
/// every point counted as used, rather than a refit on fewer points than the floor allows.
#[test]
fn clipping_keeps_the_previous_fit_below_the_point_floor() {
    let center = DVec2::new(500.0, 500.0);
    let mut ref_points: Vec<DVec2> = (0..8)
        .map(|i| {
            DVec2::new(
                100.0 + 100.0 * f64::from(i),
                200.0 + 70.0 * f64::from(i % 3),
            )
        })
        .collect();
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|&p| barrel().image(p)).collect();
    ref_points.push(DVec2::new(300.0, 800.0));
    target_points.push(DVec2::new(340.0, 760.0));

    let config = SipConfig {
        order: 2,
        reference_point: Some(center),
        ..Default::default()
    };
    let fit = fit_sip(&ref_points, &target_points, &Transform::identity(), &config);
    assert_eq!(fit.points_used, 9);
    assert_eq!(fit.points_rejected, 0);
}

/// Points on one line through the reference point have `v = 0`, so every term in `v` is a zero
/// column and the system has no unique solution: the fit is refused, not solved by a pseudo-inverse
/// that would pick one of infinitely many.
#[test]
fn a_rank_deficient_layout_is_refused() {
    let center = DVec2::new(500.0, 500.0);
    let reference: Vec<DVec2> = (0..=100)
        .map(|i| DVec2::new(10.0 * f64::from(i), 500.0))
        .collect();
    let target: Vec<DVec2> = reference
        .iter()
        .map(|&p| p + DVec2::new(0.5, 0.0))
        .collect();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };
    let error =
        SipPolynomial::fit_from_transform(&reference, &target, &Transform::identity(), &config)
            .unwrap_err();
    assert!(
        matches!(error, RegistrationError::SingularSipSystem),
        "{error:?}"
    );
}
