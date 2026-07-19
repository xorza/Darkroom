use crate::stacking::registration::distortion::sip::tests::*;

#[test]
fn test_insufficient_point_count_scales_with_order() {
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
fn test_zero_distortion_produces_zero_correction() {
    // When target = ref (no distortion), all SIP coefficients should be ~0.
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (0..=400).step_by(100) {
        for x in (0..=400).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            target_points.push(p);
        }
    }

    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::new(200.0, 200.0)),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    // Verify all coefficients are essentially zero
    for (i, &c) in sip.coeffs_u.iter().enumerate() {
        assert!(
            c.abs() < 1e-12,
            "coeffs_u[{}] should be ~0, got {:.e}",
            i,
            c
        );
    }
    for (i, &c) in sip.coeffs_v.iter().enumerate() {
        assert!(
            c.abs() < 1e-12,
            "coeffs_v[{}] should be ~0, got {:.e}",
            i,
            c
        );
    }

    // Corrections at all points should be zero
    for &p in &ref_points {
        let corrected = sip.correct(p);
        assert!(
            (corrected - p).length() < 1e-10,
            "Correction at {:?} should be zero, got {:?}",
            p,
            corrected - p
        );
    }
}

#[test]
fn test_invalid_config_returns_error() {
    let ref_points = vec![DVec2::ZERO; 10];
    let target_points = vec![DVec2::ZERO; 10];
    let transform = Transform::identity();

    for (config, expected_message) in [
        (
            SipConfig {
                order: 1,
                ..Default::default()
            },
            "SIP order must be 2-5, got 1",
        ),
        (
            SipConfig {
                order: 6,
                ..Default::default()
            },
            "SIP order must be 2-5, got 6",
        ),
        (
            SipConfig {
                clip_sigma: 0.0,
                ..Default::default()
            },
            "SIP clip_sigma must be positive and finite, got 0",
        ),
    ] {
        let error =
            SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
                .unwrap_err();

        match error {
            RegistrationError::InvalidConfig(message) => {
                assert_eq!(message, expected_message);
            }
            other => panic!("expected invalid configuration error, got {other:?}"),
        }
    }
}

#[test]
fn test_mismatched_and_singular_fits_return_exact_errors() {
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

#[test]
fn test_max_correction_at_corners() {
    // For radial distortion from center, max correction is at the corners
    // (farthest from center).
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (ref_points, target_points) = make_radial_distortion_points(center, k, 100, 1000);

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let max_corr = sip.max_correction(1000, 1000, 50.0);

    // Hand-compute expected max correction at corner (0,0):
    // d = (0,0) - (500,500) = (-500, -500), |d|^2 = 500000
    // distortion = (-500, -500) * 1e-7 * 500000 = (-25, -25)
    // |distortion| = 25 * sqrt(2) ~ 35.36
    // The SIP should recover most of this distortion.
    let expected_corner_correction = 25.0 * 2.0_f64.sqrt();

    assert!(
        max_corr > expected_corner_correction * 0.9,
        "Max correction {:.4} should be close to expected corner correction {:.4}",
        max_corr,
        expected_corner_correction
    );

    // Also verify max_correction at center region is much smaller
    let _max_corr_center = sip.max_correction(100, 100, 50.0);
    // The 100x100 grid at (0,0)-(100,100) is far from center,
    // but let's use a grid around center instead by testing correction there
    let center_correction = (sip.correct(center) - center).length();
    assert!(
        center_correction < 1e-10,
        "Correction at center should be ~0, got {:.e}",
        center_correction
    );
}

#[test]
fn test_max_correction_zero_distortion() {
    // With zero distortion, max_correction should be ~0.
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (0..=400).step_by(100) {
        for x in (0..=400).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            target_points.push(p);
        }
    }

    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::new(200.0, 200.0)),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let max_corr = sip.max_correction(400, 400, 100.0);
    assert!(
        max_corr < 1e-8,
        "Zero distortion max_correction should be ~0, got {:.e}",
        max_corr
    );
}

#[test]
fn test_compute_corrected_residuals_length() {
    let center = DVec2::new(500.0, 500.0);
    let (ref_points, target_points) = make_radial_distortion_points(center, 1e-7, 100, 1000);
    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    assert_eq!(residuals.len(), ref_points.len());
}

#[test]
fn test_compute_corrected_residuals_all_small_for_fitted_data() {
    // After fitting, every individual residual should be small (not just the mean).
    let center = DVec2::new(500.0, 500.0);
    let (ref_points, target_points) = make_radial_distortion_points(center, 1e-7, 100, 1000);
    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let max_residual = residuals.iter().cloned().fold(0.0_f64, f64::max);
    assert!(
        max_residual < 0.05,
        "Max individual residual should be small, got {:.6}",
        max_residual
    );
}

#[test]
fn test_higher_order_fits_higher_order_distortion_better() {
    // Create a distortion with a 4th-order component that order-2 cannot model.
    // distortion = d * k2 * r^2 + d * k4 * r^4
    let center = DVec2::new(500.0, 500.0);
    let k2 = 1e-7;
    let k4 = 1e-14; // 4th-order component

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (0..=1000).step_by(50) {
        for x in (0..=1000).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            let d = p - center;
            let r2 = d.length_squared();
            let r4 = r2 * r2;
            ref_points.push(p);
            target_points.push(p + d * k2 * r2 + d * k4 * r4);
        }
    }

    let transform = Transform::identity();

    let config_2 = SipConfig {
        order: 2,
        reference_point: Some(center),
        ..Default::default()
    };
    let config_4 = SipConfig {
        order: 4,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip_2 = fit_sip(&ref_points, &target_points, &transform, &config_2).polynomial;
    let sip_4 = fit_sip(&ref_points, &target_points, &transform, &config_4).polynomial;

    let rms_2 = rms(&sip_2.compute_corrected_residuals(&ref_points, &target_points, &transform));
    let rms_4 = rms(&sip_4.compute_corrected_residuals(&ref_points, &target_points, &transform));

    // Order 4 should fit the 4th-order component much better than order 2
    assert!(
        rms_4 < rms_2,
        "Order 4 RMS ({:.6}) should be less than order 2 RMS ({:.6})",
        rms_4,
        rms_2
    );
    // Order 4 should produce a reasonably tight fit
    // (SIP polynomials are not exact for r^4 terms due to cross-term modeling)
    assert!(rms_4 < 0.5, "Order 4 RMS should be small: {:.6}", rms_4);
}

#[test]
fn test_different_k_values_produce_different_corrections() {
    // Parameter sensitivity: different distortion strengths should produce
    // proportionally different corrections.
    let center = DVec2::new(500.0, 500.0);
    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let (ref_1, tgt_1) = make_radial_distortion_points(center, 1e-7, 100, 1000);
    let (ref_2, tgt_2) = make_radial_distortion_points(center, 5e-7, 100, 1000);

    let sip_1 = fit_sip(&ref_1, &tgt_1, &transform, &config).polynomial;
    let sip_2 = fit_sip(&ref_2, &tgt_2, &transform, &config).polynomial;

    // At corner (0, 0): distortion scales linearly with k.
    // k2/k1 = 5, so correction magnitude ratio should be ~5.
    let test_p = DVec2::new(0.0, 0.0);
    let corr_1 = (sip_1.correct(test_p) - test_p).length();
    let corr_2 = (sip_2.correct(test_p) - test_p).length();

    let ratio = corr_2 / corr_1;
    assert!(
        (ratio - 5.0).abs() < 0.5,
        "Correction ratio should be ~5.0 (k ratio), got {:.4}",
        ratio
    );
}
