use super::*;
use crate::registration::transform::Transform;

// ============================================================================
// Test helpers
// ============================================================================

/// Generate barrel/pincushion distortion point pairs on a grid.
///
/// Distortion model: target = p + (p - center) * k * |p - center|^2
/// - k > 0: barrel distortion (points pushed outward)
/// - k < 0: pincushion distortion (points pulled inward)
fn make_radial_distortion_points(
    center: DVec2,
    k: f64,
    grid_step: usize,
    extent: usize,
) -> (Vec<DVec2>, Vec<DVec2>) {
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (0..=extent).step_by(grid_step) {
        for x in (0..=extent).step_by(grid_step) {
            let p = DVec2::new(x as f64, y as f64);
            let d = p - center;
            let r2 = d.length_squared();
            ref_points.push(p);
            target_points.push(p + d * k * r2);
        }
    }
    (ref_points, target_points)
}

/// Compute RMS of a slice of residuals.
fn rms(residuals: &[f64]) -> f64 {
    (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len() as f64).sqrt()
}

// ============================================================================
// term_exponents tests
// ============================================================================

#[test]
fn test_term_exponents_order_2() {
    // Order 2: terms where p+q = 2 (linear terms excluded).
    // p+q=2: (2,0), (1,1), (0,2) = 3 terms.
    let terms = term_exponents(2);
    assert_eq!(terms.len(), 3);
    assert_eq!(terms[0], (2, 0)); // u^2
    assert_eq!(terms[1], (1, 1)); // u*v
    assert_eq!(terms[2], (0, 2)); // v^2
}

#[test]
fn test_term_exponents_order_3() {
    // Order 3: terms with 2 <= p+q <= 3.
    // p+q=2: (2,0), (1,1), (0,2) = 3 terms
    // p+q=3: (3,0), (2,1), (1,2), (0,3) = 4 terms
    // Total = 7 terms.
    let terms = term_exponents(3);
    assert_eq!(terms.len(), 7);
    // p+q=2 block
    assert_eq!(terms[0], (2, 0));
    assert_eq!(terms[1], (1, 1));
    assert_eq!(terms[2], (0, 2));
    // p+q=3 block
    assert_eq!(terms[3], (3, 0));
    assert_eq!(terms[4], (2, 1));
    assert_eq!(terms[5], (1, 2));
    assert_eq!(terms[6], (0, 3));
}

#[test]
fn test_term_exponents_order_4() {
    // Order 4: 2 <= p+q <= 4.
    // p+q=2: 3, p+q=3: 4, p+q=4: 5. Total = 12.
    let terms = term_exponents(4);
    assert_eq!(terms.len(), 12);
    // Spot-check the p+q=4 block starts at index 7
    assert_eq!(terms[7], (4, 0));
    assert_eq!(terms[11], (0, 4));
}

#[test]
fn test_term_exponents_order_5() {
    // Order 5: (5+1)(5+2)/2 - 3 = 21 - 3 = 18 terms.
    // p+q=2: 3, p+q=3: 4, p+q=4: 5, p+q=5: 6. Total = 18.
    let terms = term_exponents(5);
    assert_eq!(terms.len(), 18);
    // Last term should be (0, 5)
    assert_eq!(terms[17], (0, 5));
    // First term of p+q=5 block is at index 12
    assert_eq!(terms[12], (5, 0));
}

#[test]
fn test_term_exponents_all_satisfy_constraints() {
    for order in 2..=5 {
        let terms = term_exponents(order);
        for &(p, q) in terms.iter() {
            let total = p + q;
            assert!(
                total >= 2 && total <= order,
                "Order {}: term ({},{}) has p+q={} outside [2,{}]",
                order,
                p,
                q,
                total,
                order
            );
        }
    }
}

// ============================================================================
// monomial tests
// ============================================================================

#[test]
fn test_monomial_hand_computed() {
    // u^2 * v^0 = u^2
    // u=3.0, v=5.0: 3^2 * 5^0 = 9.0 * 1.0 = 9.0
    assert_eq!(monomial(3.0, 5.0, 2, 0), 9.0);

    // u^0 * v^3 = v^3
    // u=3.0, v=2.0: 3^0 * 2^3 = 1.0 * 8.0 = 8.0
    assert_eq!(monomial(3.0, 2.0, 0, 3), 8.0);

    // u^1 * v^1 = u*v
    // u=4.0, v=7.0: 4 * 7 = 28.0
    assert_eq!(monomial(4.0, 7.0, 1, 1), 28.0);

    // u^3 * v^2
    // u=2.0, v=3.0: 8.0 * 9.0 = 72.0
    assert_eq!(monomial(2.0, 3.0, 3, 2), 72.0);

    // u^0 * v^0 = 1.0 for any (u, v)
    assert_eq!(monomial(42.0, 99.0, 0, 0), 1.0);
}

#[test]
fn test_monomial_zero_input() {
    // u=0, v=0: u^p * v^q = 0 for any p>0 or q>0
    assert_eq!(monomial(0.0, 0.0, 2, 0), 0.0);
    assert_eq!(monomial(0.0, 0.0, 0, 2), 0.0);
    assert_eq!(monomial(0.0, 0.0, 1, 1), 0.0);
    // u^0 * v^0 = 1.0 even at origin
    assert_eq!(monomial(0.0, 0.0, 0, 0), 1.0);
}

#[test]
fn test_monomial_negative_input() {
    // u=-2.0, v=3.0, p=3, q=1
    // (-2)^3 * 3^1 = -8 * 3 = -24.0
    assert_eq!(monomial(-2.0, 3.0, 3, 1), -24.0);

    // u=-2.0, v=-3.0, p=2, q=2
    // (-2)^2 * (-3)^2 = 4 * 9 = 36.0
    assert_eq!(monomial(-2.0, -3.0, 2, 2), 36.0);
}

// ============================================================================
// avg_distance tests
// ============================================================================

#[test]
fn test_avg_distance_hand_computed() {
    let ref_pt = DVec2::new(0.0, 0.0);
    let points = [
        DVec2::new(3.0, 4.0),  // distance = sqrt(9+16) = 5.0
        DVec2::new(0.0, 10.0), // distance = 10.0
        DVec2::new(5.0, 0.0),  // distance = 5.0
    ];
    // avg = (5 + 10 + 5) / 3 = 20/3 = 6.666...
    let avg = avg_distance(&points, ref_pt);
    assert!((avg - 20.0 / 3.0).abs() < 1e-12);
}

#[test]
fn test_avg_distance_all_at_ref_returns_one() {
    // When all points coincide with reference, avg distance = 0 -> clamp to 1.0
    let ref_pt = DVec2::new(5.0, 5.0);
    let points = [ref_pt, ref_pt, ref_pt];
    assert_eq!(avg_distance(&points, ref_pt), 1.0);
}

#[test]
fn test_avg_distance_single_point() {
    // Single point at (6, 8) from origin (0,0): distance = sqrt(36+64) = 10.0
    // avg = 10.0 / 1 = 10.0
    let ref_pt = DVec2::ZERO;
    let points = [DVec2::new(6.0, 8.0)];
    assert!((avg_distance(&points, ref_pt) - 10.0).abs() < 1e-12);
}

// ============================================================================
// SipPolynomial::correct with known coefficients
// ============================================================================

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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

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

// ============================================================================
// fit_from_transform tests
// ============================================================================

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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

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

    let sip_barrel = SipPolynomial::fit_from_transform(&ref_b, &tgt_b, &transform, &config)
        .unwrap()
        .polynomial;
    let sip_pincushion = SipPolynomial::fit_from_transform(&ref_p, &tgt_p, &transform, &config)
        .unwrap()
        .polynomial;

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

// ============================================================================
// Edge cases and validation
// ============================================================================

#[test]
fn test_too_few_points_returns_none() {
    // Order 2 has 3 terms, minimum points = 3*3 = 9.
    // 2 points < 9 => None.
    let ref_points = vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 100.0)];
    let target_points = vec![DVec2::new(0.1, 0.0), DVec2::new(99.9, 100.0)];

    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::ZERO),
        ..Default::default()
    };

    assert!(
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
            .is_none()
    );
}

#[test]
fn test_exactly_minimum_points_returns_none() {
    // Order 2 has 3 terms. 3*3 = 9 required. Test with exactly 8 (should fail).
    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::ZERO),
        ..Default::default()
    };

    let ref_points: Vec<DVec2> = (0..8).map(|i| DVec2::new(i as f64 * 50.0, 0.0)).collect();
    let target_points = ref_points.clone();

    assert!(
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
            .is_none(),
        "8 points < 9 minimum for order 2 should return None"
    );
}

#[test]
fn test_minimum_point_count_scales_with_order() {
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

        // One fewer than minimum -> None
        let ref_pts: Vec<DVec2> = (0..min_needed - 1)
            .map(|i| {
                // Spread points on a grid to avoid singular matrix
                let row = i / 10;
                let col = i % 10;
                DVec2::new(col as f64 * 100.0, row as f64 * 100.0)
            })
            .collect();
        let tgt_pts = ref_pts.clone();

        assert!(
            SipPolynomial::fit_from_transform(&ref_pts, &tgt_pts, &transform, &config).is_none(),
            "Order {}: {} points < {} minimum should return None",
            order,
            min_needed - 1,
            min_needed
        );
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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

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
#[should_panic(expected = "SIP order must be 2-5")]
fn test_invalid_order_low() {
    let config = SipConfig {
        order: 1,
        reference_point: None,
        ..Default::default()
    };
    let ref_points = vec![DVec2::ZERO; 10];
    let target_points = vec![DVec2::ZERO; 10];
    let transform = Transform::identity();
    SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config);
}

#[test]
#[should_panic(expected = "SIP order must be 2-5")]
fn test_invalid_order_high() {
    let config = SipConfig {
        order: 6,
        reference_point: None,
        ..Default::default()
    };
    let ref_points = vec![DVec2::ZERO; 10];
    let target_points = vec![DVec2::ZERO; 10];
    let transform = Transform::identity();
    SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config);
}

#[test]
fn test_mismatched_point_counts_returns_none() {
    let config = SipConfig::default();
    let ref_points = vec![DVec2::ZERO; 30];
    let target_points = vec![DVec2::ZERO; 20];
    let transform = Transform::identity();
    let result =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config);
    assert!(result.is_none());
}

// ============================================================================
// max_correction tests
// ============================================================================

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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

    let max_corr = sip.max_correction(400, 400, 100.0);
    assert!(
        max_corr < 1e-8,
        "Zero distortion max_correction should be ~0, got {:.e}",
        max_corr
    );
}

// ============================================================================
// compute_corrected_residuals tests
// ============================================================================

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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let max_residual = residuals.iter().cloned().fold(0.0_f64, f64::max);
    assert!(
        max_residual < 0.05,
        "Max individual residual should be small, got {:.6}",
        max_residual
    );
}

// ============================================================================
// Order sensitivity tests
// ============================================================================

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

    let sip_2 =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_2)
            .unwrap()
            .polynomial;
    let sip_4 =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_4)
            .unwrap()
            .polynomial;

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

    let sip_1 = SipPolynomial::fit_from_transform(&ref_1, &tgt_1, &transform, &config)
        .unwrap()
        .polynomial;
    let sip_2 = SipPolynomial::fit_from_transform(&ref_2, &tgt_2, &transform, &config)
        .unwrap()
        .polynomial;

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

// ============================================================================
// Reference point tests
// ============================================================================

#[test]
fn test_reference_point_none_uses_centroid() {
    // When reference_point is None, centroid of ref_points is used.
    // For a symmetric grid (200..=800 step 100), centroid = (500, 500).
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (200..=800).step_by(100) {
        for x in (200..=800).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
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

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

    // Centroid of symmetric grid = center, so this should work as well as explicit center
    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let r = rms(&residuals);
    assert!(
        r < 0.01,
        "Centroid reference should produce good fit: RMS={:.6}",
        r
    );

    // The internal reference_point should be the centroid = (500, 500)
    // Verify: sum of ref_points / count:
    // x values: 200,300,...,800 (7 values), mean = (200+800)/2 = 500
    // y values: same. So centroid = (500, 500).
    assert!(
        (sip.reference_point - center).length() < 1e-10,
        "Reference point should be centroid (500,500), got {:?}",
        sip.reference_point
    );
}

#[test]
fn test_crpix_vs_centroid_when_points_are_off_center() {
    // When points are clustered in one quadrant, the centroid differs from
    // image center. Radial distortion from image center fits better with CRPIX.
    let image_center = DVec2::new(512.0, 384.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    // Points in lower-left quadrant only
    for y in (100..=350).step_by(50) {
        for x in (100..=450).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
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

    let sip_crpix =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_crpix)
            .unwrap()
            .polynomial;
    let sip_centroid = SipPolynomial::fit_from_transform(
        &ref_points,
        &target_points,
        &transform,
        &config_centroid,
    )
    .unwrap()
    .polynomial;

    let rms_crpix =
        rms(&sip_crpix.compute_corrected_residuals(&ref_points, &target_points, &transform));
    let rms_centroid =
        rms(&sip_centroid.compute_corrected_residuals(&ref_points, &target_points, &transform));

    // CRPIX should fit better since distortion originates from image_center
    assert!(
        rms_crpix < rms_centroid,
        "CRPIX RMS ({:.6}) should be less than centroid RMS ({:.6})",
        rms_crpix,
        rms_centroid
    );
    assert!(
        rms_crpix < 0.01,
        "CRPIX RMS should be very small: {:.6}",
        rms_crpix
    );
}

// ============================================================================
// Sigma-clipping tests
// ============================================================================

#[test]
fn test_sigma_clipping_rejects_outliers() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (mut ref_points, mut target_points) = make_radial_distortion_points(center, k, 100, 1000);

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
    let sip_no_clip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_no_clip)
            .unwrap()
            .polynomial;
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
    let sip_clipped =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_clipped)
            .unwrap()
            .polynomial;
    let rms_clipped = rms(&sip_clipped.compute_corrected_residuals(
        &ref_points[..n_clean],
        &target_points[..n_clean],
        &transform,
    ));

    // Clipped fit should be significantly better on clean points
    assert!(
        rms_clipped < rms_no_clip * 0.5,
        "Clipped RMS ({:.6}) should be much less than unclipped RMS ({:.6})",
        rms_clipped,
        rms_no_clip
    );

    // Clipped fit should recover near-perfect results
    assert!(
        rms_clipped < 0.01,
        "Clipped RMS should be near-zero: {:.6}",
        rms_clipped
    );
}

#[test]
fn test_sigma_clipping_no_effect_on_clean_data() {
    // With clean data, clipping should not reject anything, so results should
    // be identical with and without clipping.
    let center = DVec2::new(500.0, 500.0);
    let (ref_points, target_points) = make_radial_distortion_points(center, 1e-7, 100, 1000);

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

    let sip_clipped =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_clipped)
            .unwrap()
            .polynomial;
    let sip_no_clip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_no_clip)
            .unwrap()
            .polynomial;

    // Coefficients should be identical (clipping didn't change anything)
    for (i, (&a, &b)) in sip_clipped
        .coeffs_u
        .iter()
        .zip(sip_no_clip.coeffs_u.iter())
        .enumerate()
    {
        assert!(
            (a - b).abs() < 1e-14,
            "coeffs_u[{}]: clipped={:.e}, no_clip={:.e}",
            i,
            a,
            b
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
            "coeffs_v[{}]: clipped={:.e}, no_clip={:.e}",
            i,
            a,
            b
        );
    }
}

// ============================================================================
// Ill-conditioned system (Cholesky -> LU fallback)
// ============================================================================

#[test]
fn test_ill_conditioned_falls_back_to_lu() {
    // Narrow strip: y spans only 100px (450..550), x spans 1000px.
    // The 10:1 aspect ratio makes v-dependent monomials tiny relative to
    // u-dependent ones, creating near-singular A^T*A.
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (450..=550).step_by(10) {
        for x in (0..=1000).step_by(20) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            target_points.push(p + d * k * d.length_squared());
        }
    }
    // 11 y-values * 51 x-values = 561 points, order 5 needs 3*18=54 minimum

    let transform = Transform::identity();
    let config = SipConfig {
        order: 5,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

    // Verify corrections are reasonable within the data region (y=500 strip)
    for x_val in (0..=1000).step_by(100) {
        let p = DVec2::new(x_val as f64, 500.0);
        let corrected = sip.correct(p);
        let d = p - center;
        let r2 = d.length_squared();
        let expected_target = p + d * k * r2;
        let error = (transform.apply(corrected) - expected_target).length();
        assert!(
            error < 1.0,
            "Ill-conditioned fit error at x={}: {:.4} pixels",
            x_val,
            error
        );
    }
}

// ============================================================================
// Solver tests (Cholesky and LU)
// ============================================================================

#[test]
fn test_solve_cholesky_2x2_hand_computed() {
    // Solve [4 2; 2 3] * x = [8; 7]
    // By hand: det = 12-4 = 8
    // x = [4 2; 2 3]^-1 * [8; 7] = (1/8)*[3 -2; -2 4]*[8; 7] = (1/8)*[10; 12] = [1.25, 1.5]
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 4.0;
    a[1] = 2.0;
    a[2] = 2.0; // a[1*n+0]
    a[3] = 3.0; // a[1*n+1]

    let mut b = [0.0; MAX_TERMS];
    b[0] = 8.0;
    b[1] = 7.0;

    let x = solve_cholesky(&a, &b, n).unwrap();
    assert_eq!(x.len(), 2);
    assert!(
        (x[0] - 1.25).abs() < 1e-12,
        "x[0] = {}, expected 1.25",
        x[0]
    );
    assert!((x[1] - 1.5).abs() < 1e-12, "x[1] = {}, expected 1.5", x[1]);
}

#[test]
fn test_solve_cholesky_3x3_identity() {
    // Solve I * x = [3, 5, 7] => x = [3, 5, 7]
    let n = 3;
    let mut a = [0.0; MAX_ATA];
    a[0] = 1.0; // (0,0)
    a[n + 1] = 1.0; // (1,1)
    a[2 * n + 2] = 1.0; // (2,2)

    let mut b = [0.0; MAX_TERMS];
    b[0] = 3.0;
    b[1] = 5.0;
    b[2] = 7.0;

    let x = solve_cholesky(&a, &b, n).unwrap();
    assert_eq!(x.len(), 3);
    assert!((x[0] - 3.0).abs() < 1e-12);
    assert!((x[1] - 5.0).abs() < 1e-12);
    assert!((x[2] - 7.0).abs() < 1e-12);
}

#[test]
fn test_solve_lu_2x2_hand_computed() {
    // Same problem as Cholesky test but using LU directly.
    // [4 2; 2 3] * x = [8; 7] => x = [1.25, 1.5]
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 4.0;
    a[1] = 2.0;
    a[2] = 2.0;
    a[3] = 3.0;

    let mut b = [0.0; MAX_TERMS];
    b[0] = 8.0;
    b[1] = 7.0;

    let x = solve_lu(&a, &b, n).unwrap();
    assert_eq!(x.len(), 2);
    assert!(
        (x[0] - 1.25).abs() < 1e-12,
        "x[0] = {}, expected 1.25",
        x[0]
    );
    assert!((x[1] - 1.5).abs() < 1e-12, "x[1] = {}, expected 1.5", x[1]);
}

#[test]
fn test_solve_lu_singular_returns_none() {
    // Singular matrix: [1 2; 2 4] (row 2 = 2 * row 1)
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 1.0;
    a[1] = 2.0;
    a[2] = 2.0;
    a[3] = 4.0;

    let mut b = [0.0; MAX_TERMS];
    b[0] = 1.0;
    b[1] = 2.0;

    assert!(solve_lu(&a, &b, n).is_none());
}

#[test]
fn test_solve_lu_needs_pivoting() {
    // Matrix where first diagonal is zero, requiring pivoting:
    // [0 1; 1 0] * x = [3; 5] => x = [5, 3]
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 0.0;
    a[1] = 1.0;
    a[2] = 1.0;
    a[3] = 0.0;

    let mut b = [0.0; MAX_TERMS];
    b[0] = 3.0;
    b[1] = 5.0;

    let x = solve_lu(&a, &b, n).unwrap();
    assert_eq!(x.len(), 2);
    assert!((x[0] - 5.0).abs() < 1e-12, "x[0] = {}, expected 5.0", x[0]);
    assert!((x[1] - 3.0).abs() < 1e-12, "x[1] = {}, expected 3.0", x[1]);
}

#[test]
fn test_cholesky_falls_back_to_lu_for_non_positive_definite() {
    // Not positive definite: [1 0; 0 -1]. Cholesky should fail on diag,
    // then fall back to LU.
    // [1 0; 0 -1] * x = [2; -3] => x = [2, 3]
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 1.0;
    a[1] = 0.0;
    a[2] = 0.0;
    a[3] = -1.0;

    let mut b = [0.0; MAX_TERMS];
    b[0] = 2.0;
    b[1] = -3.0;

    let x = solve_cholesky(&a, &b, n).unwrap();
    assert_eq!(x.len(), 2);
    assert!((x[0] - 2.0).abs() < 1e-12, "x[0] = {}, expected 2.0", x[0]);
    assert!((x[1] - 3.0).abs() < 1e-12, "x[1] = {}, expected 3.0", x[1]);
}

// ============================================================================
// SipConfig validation
// ============================================================================

#[test]
fn test_sip_config_default_values() {
    let config = SipConfig::default();
    assert_eq!(config.order, 3);
    assert!(config.reference_point.is_none());
    assert!((config.clip_sigma - 3.0).abs() < 1e-15);
    assert_eq!(config.clip_iterations, 3);
}

#[test]
fn test_sip_config_validate_accepts_all_valid_orders() {
    for order in 2..=5 {
        let config = SipConfig {
            order,
            ..Default::default()
        };
        config.validate(); // Should not panic
    }
}

// ============================================================================
// Norm scale tests
// ============================================================================

#[test]
fn test_norm_scale_stored_correctly() {
    // The norm_scale should be the average distance from ref_points to reference_point.
    let center = DVec2::new(0.0, 0.0);
    let (ref_points, target_points) = make_radial_distortion_points(center, 1e-7, 100, 1000);

    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap()
        .polynomial;

    let expected_norm_scale = avg_distance(&ref_points, center);
    assert!(
        (sip.norm_scale - expected_norm_scale).abs() < 1e-10,
        "norm_scale: got {:.6}, expected {:.6}",
        sip.norm_scale,
        expected_norm_scale
    );
}

// ============================================================================
// SipFitResult quality metric tests
// ============================================================================

#[test]
fn test_fit_result_zero_distortion_metrics() {
    // Zero distortion: target == ref. All residuals and corrections should be ~0.
    // points_rejected = 0 (no outliers to clip).
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (0..=400).step_by(100) {
        for x in (0..=400).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            target_points.push(p);
        }
    }
    // 5×5 grid = 25 points. Order 2 has 3 terms, minimum = 9. 25 >= 9.
    let n = ref_points.len();
    assert_eq!(n, 25);

    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::new(200.0, 200.0)),
        ..Default::default()
    };

    let result =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
            .unwrap();

    assert_eq!(result.points_used, 25);
    assert_eq!(result.points_rejected, 0);
    assert!(
        result.rms_residual < 1e-10,
        "rms_residual should be ~0, got {:.e}",
        result.rms_residual
    );
    assert!(
        result.max_residual < 1e-10,
        "max_residual should be ~0, got {:.e}",
        result.max_residual
    );
    assert!(
        result.max_correction < 1e-10,
        "max_correction should be ~0, got {:.e}",
        result.max_correction
    );
}

#[test]
fn test_fit_result_barrel_metrics() {
    // Barrel distortion k=1e-7 on 1000×1000 grid.
    // Expected: rms_residual small, max_correction ~ 25*sqrt(2) ≈ 35.36 px at corners.
    //
    // Corner (0,0): d = (0,0)-(500,500) = (-500,-500), |d|^2 = 500000
    // distortion = (-500,-500) * 1e-7 * 500000 = (-25,-25)
    // |distortion| = 25*sqrt(2) ≈ 35.355
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (ref_points, target_points) = make_radial_distortion_points(center, k, 100, 1000);
    let n = ref_points.len(); // 11×11 = 121

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let result =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
            .unwrap();

    // Clean data → no rejections
    assert_eq!(result.points_used, n);
    assert_eq!(result.points_rejected, 0);

    // Good fit → small residuals
    assert!(
        result.rms_residual < 0.01,
        "rms_residual should be small, got {:.6}",
        result.rms_residual
    );
    assert!(
        result.max_residual < 0.05,
        "max_residual should be small, got {:.6}",
        result.max_residual
    );

    // max_correction should be close to 25*sqrt(2) = 35.355
    let expected_corner = 25.0 * 2.0_f64.sqrt();
    assert!(
        result.max_correction > expected_corner * 0.9,
        "max_correction {:.4} should be close to {:.4}",
        result.max_correction,
        expected_corner
    );
    assert!(
        result.max_correction < expected_corner * 1.1,
        "max_correction {:.4} should not overshoot {:.4}",
        result.max_correction,
        expected_corner
    );
}

#[test]
fn test_fit_result_with_outliers_metrics() {
    // Inject 3 outliers into clean barrel data. Sigma-clipping should reject exactly 3.
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (mut ref_points, mut target_points) = make_radial_distortion_points(center, k, 100, 1000);
    let n_clean = ref_points.len(); // 121

    // 3 gross outliers (20-pixel shifts)
    ref_points.push(DVec2::new(300.0, 300.0));
    target_points.push(DVec2::new(320.0, 280.0));
    ref_points.push(DVec2::new(700.0, 200.0));
    target_points.push(DVec2::new(685.0, 225.0));
    ref_points.push(DVec2::new(100.0, 800.0));
    target_points.push(DVec2::new(130.0, 810.0));

    let n_total = ref_points.len(); // 124
    assert_eq!(n_total, n_clean + 3);

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let result =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
            .unwrap();

    // Sigma-clipping should reject at least the 3 gross outliers.
    // The initial unclipped fit is contaminated, so early iterations may also
    // reject a few clean points near the outliers before converging.
    assert!(
        result.points_rejected >= 3,
        "Expected at least 3 rejections, got {}",
        result.points_rejected
    );
    assert_eq!(result.points_used + result.points_rejected, n_total);

    // After rejection, fit quality should still be good
    assert!(
        result.rms_residual < 0.01,
        "rms_residual after clipping should be small, got {:.6}",
        result.rms_residual
    );
}

#[test]
fn test_fit_result_points_used_plus_rejected_equals_total() {
    // Invariant: points_used + points_rejected == n for any fit.
    let center = DVec2::new(500.0, 500.0);
    let transform = Transform::identity();

    // Case 1: clean data, no rejections
    let (ref_1, tgt_1) = make_radial_distortion_points(center, 1e-7, 100, 1000);
    let n1 = ref_1.len();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };
    let r1 = SipPolynomial::fit_from_transform(&ref_1, &tgt_1, &transform, &config).unwrap();
    assert_eq!(
        r1.points_used + r1.points_rejected,
        n1,
        "Case 1: {} + {} != {}",
        r1.points_used,
        r1.points_rejected,
        n1
    );

    // Case 2: with outliers
    let (mut ref_2, mut tgt_2) = make_radial_distortion_points(center, 1e-7, 100, 1000);
    ref_2.push(DVec2::new(100.0, 100.0));
    tgt_2.push(DVec2::new(150.0, 50.0));
    ref_2.push(DVec2::new(900.0, 900.0));
    tgt_2.push(DVec2::new(880.0, 930.0));
    let n2 = ref_2.len();
    let r2 = SipPolynomial::fit_from_transform(&ref_2, &tgt_2, &transform, &config).unwrap();
    assert_eq!(
        r2.points_used + r2.points_rejected,
        n2,
        "Case 2: {} + {} != {}",
        r2.points_used,
        r2.points_rejected,
        n2
    );

    // Case 3: clipping disabled
    let config_no_clip = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        ..Default::default()
    };
    let r3 =
        SipPolynomial::fit_from_transform(&ref_2, &tgt_2, &transform, &config_no_clip).unwrap();
    assert_eq!(
        r3.points_used + r3.points_rejected,
        n2,
        "Case 3: {} + {} != {}",
        r3.points_used,
        r3.points_rejected,
        n2
    );
    // No clipping → no rejections
    assert_eq!(r3.points_rejected, 0);
    assert_eq!(r3.points_used, n2);
}

#[test]
fn test_fit_result_max_residual_geq_rms() {
    // Mathematical invariant: max_residual >= rms_residual always.
    // max(x_i) >= sqrt(mean(x_i^2)) because the max contributes to the sum.
    let center = DVec2::new(500.0, 500.0);
    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    // Case 1: barrel distortion (non-zero residuals)
    let (ref_pts, tgt_pts) = make_radial_distortion_points(center, 1e-7, 100, 1000);
    let r = SipPolynomial::fit_from_transform(&ref_pts, &tgt_pts, &transform, &config).unwrap();
    assert!(
        r.max_residual >= r.rms_residual,
        "max ({:.6e}) must be >= rms ({:.6e})",
        r.max_residual,
        r.rms_residual
    );

    // Case 2: 4th-order distortion with order-3 fit (larger residuals)
    let mut ref_pts2 = Vec::new();
    let mut tgt_pts2 = Vec::new();
    for y in (0..=1000).step_by(50) {
        for x in (0..=1000).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            let d = p - center;
            let r2 = d.length_squared();
            ref_pts2.push(p);
            tgt_pts2.push(p + d * 1e-14 * r2 * r2); // r^4 distortion
        }
    }
    let r2 = SipPolynomial::fit_from_transform(&ref_pts2, &tgt_pts2, &transform, &config).unwrap();
    assert!(
        r2.max_residual >= r2.rms_residual,
        "max ({:.6e}) must be >= rms ({:.6e})",
        r2.max_residual,
        r2.rms_residual
    );
    // Order 3 cannot fully model r^4 distortion, so residuals should be non-trivial
    assert!(
        r2.rms_residual > 1e-6,
        "r^4 distortion with order-3 fit should have non-trivial residuals: {:.6e}",
        r2.rms_residual
    );
}

#[test]
fn test_fit_result_higher_order_reduces_residuals() {
    // For distortion with a 4th-order component, order-4 fit should produce
    // strictly lower residuals than order-3 fit.
    //
    // Distortion: d * k2 * r^2 + d * k4 * r^4
    // Order 3 can model r^2 (terms u^2, uv, v^2 capture it) but NOT r^4.
    // Order 4 adds terms like u^4, u^3v, etc. that can partially model r^4.
    let center = DVec2::new(500.0, 500.0);
    let k2 = 1e-7;
    let k4 = 1e-14;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (0..=1000).step_by(50) {
        for x in (0..=1000).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            let d = p - center;
            let r2 = d.length_squared();
            ref_points.push(p);
            target_points.push(p + d * k2 * r2 + d * k4 * r2 * r2);
        }
    }

    let transform = Transform::identity();

    // Disable sigma-clipping so both orders fit all points.
    // Otherwise order 3 rejects many points (it can't model r^4)
    // and the comparison becomes apples-to-oranges.
    let config_3 = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        ..Default::default()
    };
    let config_4 = SipConfig {
        order: 4,
        reference_point: Some(center),
        clip_iterations: 0,
        ..Default::default()
    };

    let r3 = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_3)
        .unwrap();
    let r4 = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_4)
        .unwrap();

    // Order 4 should produce strictly lower RMS residual
    assert!(
        r4.rms_residual < r3.rms_residual,
        "Order 4 rms ({:.6e}) should be < order 3 rms ({:.6e})",
        r4.rms_residual,
        r3.rms_residual
    );
    // max_residual may be equal if both orders have the same worst-case point,
    // but order 4 should be at most as bad
    assert!(
        r4.max_residual <= r3.max_residual,
        "Order 4 max ({:.6e}) should be <= order 3 max ({:.6e})",
        r4.max_residual,
        r3.max_residual
    );

    // Both use all points (clipping disabled)
    let n = ref_points.len();
    assert_eq!(r3.points_used, n);
    assert_eq!(r4.points_used, n);
    assert_eq!(r3.points_rejected, 0);
    assert_eq!(r4.points_rejected, 0);

    // Order 4 max_correction should also differ (more terms, tighter fit)
    // Both should capture roughly the same distortion magnitude
    let expected_corner = 25.0 * 2.0_f64.sqrt(); // r^2 component dominates
    assert!(
        r3.max_correction > expected_corner * 0.8,
        "Order 3 max_correction {:.4} too small",
        r3.max_correction
    );
    assert!(
        r4.max_correction > expected_corner * 0.8,
        "Order 4 max_correction {:.4} too small",
        r4.max_correction
    );
}

#[test]
fn test_fit_result_max_correction_hand_computed() {
    // Barrel distortion k=1e-7 centered at (500,500), grid points at multiples of 100.
    //
    // The farthest grid point from center is a corner, e.g. (0, 0):
    //   d = (-500, -500), |d|^2 = 500000
    //   distortion = d * k * |d|^2 = (-500, -500) * 1e-7 * 500000 = (-25, -25)
    //   |distortion| = 25 * sqrt(2) = 35.3553...
    //
    // SIP order 3 models radial distortion exactly (r^2 terms are degree 3 in x,y
    // when multiplied by d). So the correction at the corner should match the
    // distortion magnitude closely.
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (ref_points, target_points) = make_radial_distortion_points(center, k, 100, 1000);

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };

    let result =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
            .unwrap();

    // Expected max correction at corner:
    // |d| * k * |d|^2 = sqrt(500000) * 1e-7 * 500000 = 707.107 * 0.05 = 35.3553
    let expected = 25.0 * 2.0_f64.sqrt(); // = 35.35533...
    assert!(
        (result.max_correction - expected).abs() < 0.1,
        "max_correction={:.4}, expected={:.4}, diff={:.6}",
        result.max_correction,
        expected,
        (result.max_correction - expected).abs()
    );
}

#[test]
fn test_fit_result_clipping_disabled_vs_enabled_metrics() {
    // With 3 outliers, clipping=off should have:
    //   - points_rejected = 0 (no clipping)
    //   - higher rms_residual than clipping=on (outliers pull the fit)
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (mut ref_points, mut target_points) = make_radial_distortion_points(center, k, 100, 1000);

    // Inject outliers
    ref_points.push(DVec2::new(300.0, 300.0));
    target_points.push(DVec2::new(320.0, 280.0));
    ref_points.push(DVec2::new(700.0, 200.0));
    target_points.push(DVec2::new(685.0, 225.0));
    ref_points.push(DVec2::new(100.0, 800.0));
    target_points.push(DVec2::new(130.0, 810.0));
    let n = ref_points.len();

    let transform = Transform::identity();

    let config_clip = SipConfig {
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

    let r_clip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_clip)
            .unwrap();
    let r_no_clip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_no_clip)
            .unwrap();

    // No-clip: all points used
    assert_eq!(r_no_clip.points_used, n);
    assert_eq!(r_no_clip.points_rejected, 0);

    // Clip: some points rejected
    assert!(r_clip.points_rejected >= 3);
    assert!(r_clip.points_used < n);

    // Clipped fit should have strictly lower rms on its surviving points
    // than the unclipped fit (which is polluted by outliers)
    assert!(
        r_clip.rms_residual < r_no_clip.rms_residual,
        "Clipped rms ({:.6e}) should be < unclipped rms ({:.6e})",
        r_clip.rms_residual,
        r_no_clip.rms_residual
    );
}
