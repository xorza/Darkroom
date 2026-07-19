use crate::stacking::registration::distortion::sip::tests::*;

#[test]
fn test_correct_at_reference_point_is_identity() {
    // At the reference point, u=0, v=0, so all monomials with p+q >= 2 are zero.
    // correction_at(ref) = (0, 0), so correct(ref) = ref.
    let center = DVec2::new(500.0, 500.0);
    let (ref_points, target_points) = make_radial_distortion_points(center, 1e-7, 100, 1000);

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let corrected = sip.correct(center);
    // At center: u=0, v=0. All monomials u^p*v^q with p+q>=2 evaluate to 0.
    // So correction = (0, 0) and corrected = center exactly.
    assert!(
        (corrected - center).length() < 1e-12,
        "Correction at reference point should be zero, got {:?}",
        corrected - center
    );
}

#[test]
fn test_correct_barrel_at_specific_point() {
    // Barrel distortion: target = p + (p-center)*k*|p-center|^2
    // After SIP fit, correct(p) should produce a point that maps close to target.
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

    // Test point (700, 300): d = (200, -200), |d|^2 = 80000
    // Expected distortion: (200, -200) * 1e-7 * 80000 = (1.6, -1.6)
    // Expected target: (701.6, 298.4)
    // The SIP correction should move (700, 300) so that transform(correct(700,300)) ~ (701.6, 298.4)
    let test_p = DVec2::new(700.0, 300.0);
    let corrected = sip.correct(test_p);
    let mapped = transform.apply(corrected);
    let d = test_p - center; // (200, -200)
    let r2 = d.length_squared(); // 80000
    let expected_target = test_p + d * k * r2; // (701.6, 298.4)
    let error = (mapped - expected_target).length();
    assert!(
        error < 0.01,
        "At (700,300): mapped={:?}, expected_target={:?}, error={:.6}",
        mapped,
        expected_target,
        error
    );

    // Also verify the correction direction is outward (barrel pushes outward)
    let correction = corrected - test_p;
    // d = (200, -200), so correction should be roughly in the same direction
    assert!(
        correction.x > 0.0,
        "Correction x should be positive (outward)"
    );
    assert!(
        correction.y < 0.0,
        "Correction y should be negative (outward)"
    );
}

#[test]
fn test_fit_barrel_distortion_with_translation() {
    // Tests that SIP fitting works when the transform includes a translation.
    // target = p + translation + (p-center)*k*|p-center|^2
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let translation = DVec2::new(10.0, 5.0);

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (100..=900).step_by(100) {
        for x in (100..=900).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            let r2 = d.length_squared();
            target_points.push(p + translation + d * k * r2);
        }
    }

    let transform = Transform::translation(translation);
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let r = rms(&residuals);
    assert!(
        r < 0.01,
        "RMS after SIP correction with translation: {:.6}",
        r
    );
}

#[test]
fn test_fit_pincushion_distortion() {
    // Pincushion (k < 0): points are pulled inward toward center.
    let center = DVec2::new(512.0, 384.0);
    let k = -5e-8;
    let (ref_points, target_points) = make_radial_distortion_points(center, k, 100, 1000);

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let r = rms(&residuals);
    assert!(r < 0.01, "Pincushion RMS: {:.6}", r);

    // Verify sign: pincushion correction at corner should pull inward
    // Point (900, 700): d = (388, 316), correction should be in negative d direction
    let corner = DVec2::new(900.0, 700.0);
    let correction = sip.correct(corner) - corner;
    let d = corner - center;
    // For pincushion (k<0), targets are closer to center than ref points.
    // SIP corrects for this by pushing outward (opposite sign to the distortion).
    // distortion = d * k * r2 with k<0, so distortion is inward.
    // Correction should push outward to compensate, i.e., same direction as d.
    // But actually the SIP fits the residual -(transform(ref) - target) = target - transform(ref).
    // With identity transform: residual = target - ref = d * k * r2 (inward for k<0).
    // Correction = residual direction = inward. Let's just verify small residuals and direction.
    let dot = correction.x * d.x + correction.y * d.y;
    assert!(
        dot < 0.0,
        "Pincushion correction should be inward (toward center): correction={:?}, d={:?}",
        correction,
        d
    );
}

#[test]
fn test_barrel_vs_pincushion_opposite_corrections() {
    // Barrel (k>0) and pincushion (k<0) should produce corrections in opposite directions.
    let center = DVec2::new(500.0, 500.0);
    let transform = Transform::identity();

    let (ref_b, tgt_b) = make_radial_distortion_points(center, 1e-7, 100, 1000);
    let (ref_p, tgt_p) = make_radial_distortion_points(center, -1e-7, 100, 1000);

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip_barrel = fit_sip(&ref_b, &tgt_b, &transform, &config).polynomial;
    let sip_pincushion = fit_sip(&ref_p, &tgt_p, &transform, &config).polynomial;

    // Test at a corner point: corrections should have opposite signs
    let test_p = DVec2::new(800.0, 200.0);
    let corr_barrel = sip_barrel.correct(test_p) - test_p;
    let corr_pincushion = sip_pincushion.correct(test_p) - test_p;

    // The dot product of opposite corrections should be negative
    let dot = corr_barrel.x * corr_pincushion.x + corr_barrel.y * corr_pincushion.y;
    assert!(
        dot < 0.0,
        "Barrel and pincushion corrections should be opposite: barrel={:?}, pincushion={:?}, dot={:.6}",
        corr_barrel,
        corr_pincushion,
        dot
    );

    // Magnitudes should be similar (same |k|)
    let mag_barrel = corr_barrel.length();
    let mag_pincushion = corr_pincushion.length();
    assert!(
        (mag_barrel - mag_pincushion).abs() / mag_barrel < 0.01,
        "Magnitudes should be similar: barrel={:.4}, pincushion={:.4}",
        mag_barrel,
        mag_pincushion
    );
}
