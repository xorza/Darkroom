use super::*;

#[test]
fn reference_point_none_uses_centroid() {
    // When reference_point is None, centroid of ref_points is used.
    // For a symmetric grid (200..=800 step 100), centroid = (500, 500).
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (200..=800).step_by(100) {
        for x in (200..=800).step_by(100) {
            let p = DVec2::new(f64::from(x), f64::from(y));
            ref_points.push(p);
            let d = p - center;
            target_points.push(p + d * k * d.length_squared());
        }
    }

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: None,
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    // Centroid of symmetric grid = center, so this should work as well as explicit center
    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let r = rms(&residuals);
    assert!(
        r < 0.01,
        "Centroid reference should produce good fit: RMS={r:.6}"
    );

    // The internal reference_point should be the centroid = (500, 500)
    // Verify: sum of ref_points / count:
    // x values: 200,300,...,800 (7 values), mean = (200+800)/2 = 500
    // y values: same. So centroid = (500, 500).
    assert_eq!(
        sip.norm,
        PointNormalization::around(&ref_points, center),
        "the reference point is the centroid (500, 500)"
    );
}

#[test]
fn crpix_vs_centroid_when_points_are_off_center() {
    // When points are clustered in one quadrant, the centroid differs from
    // image center. Radial distortion from image center fits better with CRPIX.
    let image_center = DVec2::new(512.0, 384.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    // Points in lower-left quadrant only
    for y in (100..=350).step_by(50) {
        for x in (100..=450).step_by(50) {
            let p = DVec2::new(f64::from(x), f64::from(y));
            ref_points.push(p);
            let d = p - image_center;
            target_points.push(p + d * k * d.length_squared());
        }
    }

    let transform = Transform::identity();

    let config_crpix = SipConfig {
        order: 3,
        reference_point: Some(image_center),
        ..Default::default()
    };
    let config_centroid = SipConfig {
        order: 3,
        reference_point: None,
        ..Default::default()
    };

    let sip_crpix = fit_sip(&ref_points, &target_points, &transform, &config_crpix).polynomial;
    let sip_centroid =
        fit_sip(&ref_points, &target_points, &transform, &config_centroid).polynomial;

    let rms_crpix =
        rms(&sip_crpix.compute_corrected_residuals(&ref_points, &target_points, &transform));
    let rms_centroid =
        rms(&sip_centroid.compute_corrected_residuals(&ref_points, &target_points, &transform));

    // CRPIX should fit better since distortion originates from image_center
    assert!(
        rms_crpix < rms_centroid,
        "CRPIX RMS ({rms_crpix:.6}) should be less than centroid RMS ({rms_centroid:.6})"
    );
    assert!(
        rms_crpix < 0.01,
        "CRPIX RMS should be very small: {rms_crpix:.6}"
    );
}

#[test]
fn sigma_clipping_rejects_outliers() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let PointPairs {
        reference: mut ref_points,
        target: mut target_points,
    } = make_radial_distortion_points(center, k, 100, 1000);

    let transform = Transform::identity();
    let n_clean = ref_points.len();

    // Inject 3 gross outliers (20-pixel shifts)
    ref_points.push(DVec2::new(300.0, 300.0));
    target_points.push(DVec2::new(320.0, 280.0));
    ref_points.push(DVec2::new(700.0, 200.0));
    target_points.push(DVec2::new(685.0, 225.0));
    ref_points.push(DVec2::new(100.0, 800.0));
    target_points.push(DVec2::new(130.0, 810.0));

    // Fit WITHOUT clipping
    let config_no_clip = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        ..Default::default()
    };
    let sip_no_clip = fit_sip(&ref_points, &target_points, &transform, &config_no_clip).polynomial;
    let rms_no_clip = rms(&sip_no_clip.compute_corrected_residuals(
        &ref_points[..n_clean],
        &target_points[..n_clean],
        &transform,
    ));

    // Fit WITH clipping (default: sigma=3, iterations=3)
    let config_clipped = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };
    let sip_clipped = fit_sip(&ref_points, &target_points, &transform, &config_clipped).polynomial;
    let rms_clipped = rms(&sip_clipped.compute_corrected_residuals(
        &ref_points[..n_clean],
        &target_points[..n_clean],
        &transform,
    ));

    // Clipped fit should be significantly better on clean points
    assert!(
        rms_clipped < rms_no_clip * 0.5,
        "Clipped RMS ({rms_clipped:.6}) should be much less than unclipped RMS ({rms_no_clip:.6})"
    );

    // Clipped fit should recover near-perfect results
    assert!(
        rms_clipped < 0.01,
        "Clipped RMS should be near-zero: {rms_clipped:.6}"
    );
}

#[test]
fn sigma_clipping_no_effect_on_clean_data() {
    // With clean data, clipping should not reject anything, so results should
    // be identical with and without clipping.
    let center = DVec2::new(500.0, 500.0);
    let PointPairs {
        reference: ref_points,
        target: target_points,
    } = make_radial_distortion_points(center, 1e-7, 100, 1000);

    let transform = Transform::identity();

    let config_clipped = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };
    let config_no_clip = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        ..Default::default()
    };

    let sip_clipped = fit_sip(&ref_points, &target_points, &transform, &config_clipped).polynomial;
    let sip_no_clip = fit_sip(&ref_points, &target_points, &transform, &config_no_clip).polynomial;

    // Coefficients should be identical (clipping didn't change anything)
    for (i, (&a, &b)) in sip_clipped
        .coeffs_u
        .iter()
        .zip(sip_no_clip.coeffs_u.iter())
        .enumerate()
    {
        assert!(
            (a - b).abs() < 1e-14,
            "coeffs_u[{i}]: clipped={a:.e}, no_clip={b:.e}"
        );
    }
    for (i, (&a, &b)) in sip_clipped
        .coeffs_v
        .iter()
        .zip(sip_no_clip.coeffs_v.iter())
        .enumerate()
    {
        assert!(
            (a - b).abs() < 1e-14,
            "coeffs_v[{i}]: clipped={a:.e}, no_clip={b:.e}"
        );
    }
}

/// A narrow strip — x over 1000 px, y over 100 — makes the v-dependent monomials tiny beside the
/// u-dependent ones, and its normal equations would square that conditioning. The SVD solve on the
/// design matrix keeps it, so the exact cubic field is recovered to rounding inside the strip:
/// coordinates of order 10³ resolved to `u·10³` ≈ 1e-13, under 1e-9 px with the design's
/// conditioning on top.
#[test]
fn a_narrow_strip_is_fitted_to_rounding() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (450..=550).step_by(10) {
        for x in (0..=1000).step_by(20) {
            let p = DVec2::new(f64::from(x), f64::from(y));
            ref_points.push(p);
            let d = p - center;
            target_points.push(p + d * k * d.length_squared());
        }
    }
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
        let d = p - center;
        let expected_target = p + d * k * d.length_squared();
        let error = (transform.apply(sip.correct(p)) - expected_target).length();
        assert!(error < 1e-9, "strip fit error at x={x_val}: {error:e} px");
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
    let mut target_points: Vec<DVec2> = ref_points
        .iter()
        .map(|&p| {
            let d = p - center;
            p + d * 1e-7 * d.length_squared()
        })
        .collect();
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
