use super::*;

#[test]
fn test_num_terms() {
    assert_eq!(num_terms(2), 3); // u², uv, v²
    assert_eq!(num_terms(3), 7); // + u³, u²v, uv², v³
    assert_eq!(num_terms(4), 12); // + u⁴, u³v, u²v², uv³, v⁴
    assert_eq!(num_terms(5), 18); // + u⁵, u⁴v, u³v², u²v³, uv⁴, v⁵
}

#[test]
fn test_term_exponents_order2() {
    let terms = term_exponents(2);
    assert_eq!(terms.len(), 3);
    // For total=2: (2,0), (1,1), (0,2)
    assert_eq!(terms[0], (2, 0));
    assert_eq!(terms[1], (1, 1));
    assert_eq!(terms[2], (0, 2));
}

#[test]
fn test_term_exponents_order3() {
    let terms = term_exponents(3);
    assert_eq!(terms.len(), 7);
    // total=2: (2,0), (1,1), (0,2)
    // total=3: (3,0), (2,1), (1,2), (0,3)
    assert_eq!(terms[3], (3, 0));
    assert_eq!(terms[4], (2, 1));
    assert_eq!(terms[5], (1, 2));
    assert_eq!(terms[6], (0, 3));
}

#[test]
fn test_sip_fit_barrel_distortion() {
    // Simulate barrel distortion: r' = r(1 + k*r²)
    // Residual dx = k * u * (u² + v²) = k*(u³ + u*v²) — these are ORDER 3 terms.
    // Order 3 SIP has terms u³, u²v, uv², v³ which can capture this exactly.
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (100..=900).step_by(100) {
        for x in (100..=900).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);

            let d = p - center;
            let r2 = d.length_squared();
            let shift = d * k * r2;
            residuals.push(shift);
        }
    }

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    // After correction, residuals should be much smaller
    for (i, &p) in ref_points.iter().enumerate() {
        let correction = sip.correction_at(p);
        let remaining = residuals[i] + correction;
        assert!(
            remaining.length() < 0.01,
            "Point {}: remaining residual {:.6} too large (correction={:?}, residual={:?})",
            i,
            remaining.length(),
            correction,
            residuals[i]
        );
    }
}

#[test]
fn test_sip_fit_pincushion_distortion() {
    // Pincushion is also cubic: dx = k * u * r², order 3 needed.
    let center = DVec2::new(512.0, 384.0);
    let k = -5e-8;

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (50..=750).step_by(100) {
        for x in (50..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);

            let d = p - center;
            let r2 = d.length_squared();
            residuals.push(d * k * r2);
        }
    }

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    for (i, &p) in ref_points.iter().enumerate() {
        let correction = sip.correction_at(p);
        let remaining = residuals[i] + correction;
        assert!(
            remaining.length() < 0.01,
            "Point {}: remaining residual {:.6}",
            i,
            remaining.length()
        );
    }
}

#[test]
fn test_sip_order2_quadratic_distortion() {
    // Order 2 SIP has terms u², uv, v². Test with a purely quadratic distortion
    // that order 2 *can* capture: dx = a*u² + b*v², dy = c*u² + d*v²
    let center = DVec2::new(500.0, 500.0);

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (100..=900).step_by(100) {
        for x in (100..=900).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);

            let u = p.x - center.x;
            let v = p.y - center.y;
            // Purely quadratic residuals
            let dx = 1e-5 * u * u + 5e-6 * v * v;
            let dy = 3e-6 * u * u + 8e-6 * v * v;
            residuals.push(DVec2::new(dx, dy));
        }
    }

    let config = SipConfig {
        order: 2,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    for (i, &p) in ref_points.iter().enumerate() {
        let correction = sip.correction_at(p);
        let remaining = residuals[i] + correction;
        assert!(
            remaining.length() < 0.01,
            "Point {}: remaining residual {:.6}",
            i,
            remaining.length()
        );
    }
}

#[test]
fn test_sip_order3_mustache_distortion() {
    // Mustache distortion: combines barrel (r²) with higher-order (r⁴).
    // dx = k2 * u * r² + k3 * u * r⁴
    // The r² term is cubic (order 3), the r⁴ term is quintic (order 5).
    // Order 5 SIP should capture both; order 3 captures only the r² part.
    let center = DVec2::new(500.0, 500.0);
    let k2 = 1e-7;
    let k3 = -2e-11;

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (50..=950).step_by(75) {
        for x in (50..=950).step_by(75) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);

            let d = p - center;
            let r2 = d.length_squared();
            let r4 = r2 * r2;
            residuals.push(d * (k2 * r2 + k3 * r4));
        }
    }

    // Use order 5 to capture the r⁴ (quintic) term
    let config = SipConfig {
        order: 5,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    let mut max_remaining = 0.0f64;
    for (i, &p) in ref_points.iter().enumerate() {
        let correction = sip.correction_at(p);
        let remaining = (residuals[i] + correction).length();
        max_remaining = max_remaining.max(remaining);
    }

    // Order 5 should capture both components well
    assert!(
        max_remaining < 0.1,
        "Max remaining residual {:.6} too large",
        max_remaining
    );
}

#[test]
fn test_sip_zero_residuals() {
    // If there's no distortion, SIP should produce near-zero corrections
    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (0..=400).step_by(100) {
        for x in (0..=400).step_by(100) {
            ref_points.push(DVec2::new(x as f64, y as f64));
            residuals.push(DVec2::ZERO);
        }
    }

    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::new(200.0, 200.0)),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    // All corrections should be near zero
    for &p in &ref_points {
        let c = sip.correction_at(p);
        assert!(c.length() < 1e-10, "Correction should be zero, got {:?}", c);
    }
}

#[test]
fn test_sip_too_few_points() {
    let ref_points = vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 100.0)];
    let residuals = vec![DVec2::new(0.1, 0.0), DVec2::new(-0.1, 0.0)];

    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::ZERO),
    };

    // 2 points < 3 terms for order 2 => should return None
    assert!(SipPolynomial::fit_residuals(&ref_points, &residuals, &config).is_none());
}

#[test]
fn test_sip_correct_applies_offset() {
    // Barrel distortion residuals are cubic, need order 3
    let center = DVec2::new(500.0, 500.0);

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    let k = 1e-6;

    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            let r2 = d.length_squared();
            residuals.push(d * k * r2);
        }
    }

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    // correct() should return p + correction
    let test_p = DVec2::new(700.0, 300.0);
    let corrected = sip.correct(test_p);
    let correction = sip.correction_at(test_p);
    assert!((corrected - test_p - correction).length() < 1e-10);
}

#[test]
fn test_sip_correct_points_batch() {
    let center = DVec2::new(500.0, 500.0);

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    let k = 1e-7;

    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            residuals.push(d * k * d.length_squared());
        }
    }

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    let test_points = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(500.0, 500.0),
        DVec2::new(900.0, 900.0),
    ];

    let batch = sip.correct_points(&test_points);
    for (i, &p) in test_points.iter().enumerate() {
        let single = sip.correct(p);
        assert!(
            (batch[i] - single).length() < 1e-10,
            "Batch vs single mismatch at point {}",
            i
        );
    }
}

#[test]
fn test_sip_fit_from_transform() {
    use crate::registration::transform::Transform;

    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    // Create points with barrel distortion (cubic residuals, need order 3)
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (100..=900).step_by(100) {
        for x in (100..=900).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);

            let d = p - center;
            let r2 = d.length_squared();
            let distortion = d * k * r2;
            target_points.push(p + DVec2::new(10.0, 5.0) + distortion);
        }
    }

    let transform = Transform::translation(DVec2::new(10.0, 5.0));

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap();

    // Corrected residuals should be small
    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let rms: f64 = (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len() as f64).sqrt();
    assert!(rms < 0.01, "RMS after SIP correction: {:.6}", rms);
}

#[test]
fn test_sip_max_correction() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            residuals.push(d * k * d.length_squared());
        }
    }

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    let max_corr = sip.max_correction(1000, 1000, 50.0);
    assert!(max_corr > 0.0, "Max correction should be positive");
}

#[test]
fn test_sip_auto_reference_point() {
    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            residuals.push(DVec2::ZERO);
        }
    }

    let config = SipConfig {
        order: 2,
        reference_point: None, // Auto-compute
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    // Reference point should be the centroid
    assert!((sip.reference_point().x - 500.0).abs() < 1e-10);
    assert!((sip.reference_point().y - 500.0).abs() < 1e-10);
}

#[test]
#[should_panic(expected = "SIP order must be 2-5")]
fn test_sip_invalid_order_low() {
    let config = SipConfig {
        order: 1,
        reference_point: None,
    };
    config.validate();
}

#[test]
#[should_panic(expected = "SIP order must be 2-5")]
fn test_sip_invalid_order_high() {
    let config = SipConfig {
        order: 6,
        reference_point: None,
    };
    config.validate();
}

#[test]
fn test_sip_order_and_coeffs_accessors() {
    let center = DVec2::new(500.0, 500.0);
    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            ref_points.push(DVec2::new(x as f64, y as f64));
            residuals.push(DVec2::ZERO);
        }
    }

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();

    assert_eq!(sip.order(), 3);
    assert_eq!(sip.coeffs_u().len(), 7); // num_terms(3) = 7
    assert_eq!(sip.coeffs_v().len(), 7);
    assert_eq!(sip.reference_point(), center);
}

#[test]
fn test_sip_all_orders() {
    // Verify that fitting works for all supported orders
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();

    for y in (0..=1000).step_by(50) {
        for x in (0..=1000).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            residuals.push(d * k * d.length_squared());
        }
    }

    for order in 2..=5 {
        let config = SipConfig {
            order,
            reference_point: Some(center),
        };

        let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config);
        assert!(sip.is_some(), "SIP order {} should fit successfully", order);

        let sip = sip.unwrap();
        assert_eq!(sip.coeffs_u().len(), num_terms(order));
        assert_eq!(sip.coeffs_v().len(), num_terms(order));
    }
}

#[test]
fn test_solve_symmetric_positive_identity() {
    // Solve I * x = b => x = b
    let a = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let b = vec![3.0, 5.0, 7.0];
    let x = solve_symmetric_positive(&a, &b, 3).unwrap();
    assert!((x[0] - 3.0).abs() < 1e-10);
    assert!((x[1] - 5.0).abs() < 1e-10);
    assert!((x[2] - 7.0).abs() < 1e-10);
}

#[test]
fn test_solve_lu_fallback() {
    // A matrix that is not positive definite but still invertible
    let a = vec![1.0, 2.0, 2.0, 1.0];
    let b = vec![5.0, 4.0];
    let x = solve_lu(&a, &b, 2).unwrap();
    // 1*x0 + 2*x1 = 5, 2*x0 + 1*x1 = 4 => x0=1, x1=2
    assert!((x[0] - 1.0).abs() < 1e-10);
    assert!((x[1] - 2.0).abs() < 1e-10);
}

// --- Inverse polynomial tests ---

/// Helper: create a SipPolynomial with barrel distortion (cubic, order 3).
fn make_barrel_sip(center: DVec2, k: f64) -> SipPolynomial {
    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    for y in (0..=1000).step_by(50) {
        for x in (0..=1000).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            residuals.push(d * k * d.length_squared());
        }
    }
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };
    SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap()
}

#[test]
fn test_inverse_roundtrip_barrel() {
    let center = DVec2::new(500.0, 500.0);
    let mut sip = make_barrel_sip(center, 1e-8);
    let max_err = sip.compute_inverse(1000, 1000);
    assert!(
        max_err < 0.1,
        "barrel roundtrip error {:.6} >= 0.1",
        max_err
    );

    // Spot-check a few points
    for &p in &[
        DVec2::new(200.0, 300.0),
        DVec2::new(800.0, 700.0),
        DVec2::new(500.0, 500.0),
    ] {
        let corrected = sip.correct(p);
        let roundtrip = sip.inverse_correct(corrected);
        let err = (roundtrip - p).length();
        assert!(err < 0.1, "roundtrip error {:.6} at {:?}", err, p);
    }
}

#[test]
fn test_inverse_roundtrip_pincushion() {
    let center = DVec2::new(512.0, 384.0);
    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    let k = -5e-9;
    for y in (0..=768).step_by(48) {
        for x in (0..=1024).step_by(48) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            residuals.push(d * k * d.length_squared());
        }
    }
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };
    let mut sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();
    let max_err = sip.compute_inverse(1024, 768);
    assert!(
        max_err < 0.1,
        "pincushion roundtrip error {:.6} >= 0.1",
        max_err
    );
}

#[test]
fn test_inverse_roundtrip_order2() {
    let center = DVec2::new(500.0, 500.0);
    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let u = p.x - center.x;
            let v = p.y - center.y;
            residuals.push(DVec2::new(
                1e-5 * u * u + 5e-6 * v * v,
                3e-6 * u * u + 8e-6 * v * v,
            ));
        }
    }
    let config = SipConfig {
        order: 2,
        reference_point: Some(center),
    };
    let mut sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();
    let max_err = sip.compute_inverse(1000, 1000);
    assert!(
        max_err < 0.1,
        "order2 roundtrip error {:.6} >= 0.1",
        max_err
    );
}

#[test]
fn test_inverse_roundtrip_mustache() {
    // Mustache distortion: barrel (r²) + higher-order (r⁴) with opposite signs.
    // Max correction ~2px at corners — realistic for wide-field imaging.
    let center = DVec2::new(500.0, 500.0);
    let k2 = 5e-9;
    let k3 = -1e-14;
    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    for y in (0..=1000).step_by(50) {
        for x in (0..=1000).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            let r2 = d.length_squared();
            residuals.push(d * (k2 * r2 + k3 * r2 * r2));
        }
    }
    let config = SipConfig {
        order: 5,
        reference_point: Some(center),
    };
    let mut sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();
    let max_err = sip.compute_inverse(1000, 1000);
    assert!(
        max_err < 0.5,
        "mustache roundtrip error {:.6} >= 0.5",
        max_err
    );
}

#[test]
fn test_inverse_zero_distortion() {
    let center = DVec2::new(500.0, 500.0);
    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            ref_points.push(DVec2::new(x as f64, y as f64));
            residuals.push(DVec2::ZERO);
        }
    }
    let config = SipConfig {
        order: 2,
        reference_point: Some(center),
    };
    let mut sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();
    let max_err = sip.compute_inverse(1000, 1000);
    assert!(
        max_err < 1e-10,
        "zero distortion roundtrip error {:.6e}",
        max_err
    );
}

#[test]
fn test_inverse_has_inverse() {
    let center = DVec2::new(500.0, 500.0);
    let mut sip = make_barrel_sip(center, 1e-8);
    assert!(!sip.has_inverse());
    sip.compute_inverse(1000, 1000);
    assert!(sip.has_inverse());
}

#[test]
#[should_panic(expected = "inverse polynomial not computed")]
fn test_inverse_panics_without_compute() {
    let center = DVec2::new(500.0, 500.0);
    let sip = make_barrel_sip(center, 1e-8);
    sip.inverse_correction_at(DVec2::new(100.0, 100.0));
}

#[test]
fn test_inverse_accessors() {
    let center = DVec2::new(500.0, 500.0);
    let mut sip = make_barrel_sip(center, 1e-8);
    sip.compute_inverse(1000, 1000);

    // Forward order 3 → inverse order 4
    assert_eq!(sip.inv_order(), 4);
    assert_eq!(sip.inv_coeffs_u().len(), num_terms(4));
    assert_eq!(sip.inv_coeffs_v().len(), num_terms(4));
}

#[test]
fn test_inverse_all_orders() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-8;

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    for y in (0..=1000).step_by(50) {
        for x in (0..=1000).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            residuals.push(d * k * d.length_squared());
        }
    }

    for order in 2..=5 {
        let config = SipConfig {
            order,
            reference_point: Some(center),
        };
        let mut sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();
        let max_err = sip.compute_inverse(1000, 1000);
        let expected_inv_order = (order + 1).min(5);
        assert_eq!(sip.inv_order(), expected_inv_order);
        assert!(
            max_err < 0.5,
            "order {} roundtrip error {:.6} >= 0.5",
            order,
            max_err
        );
    }
}

#[test]
fn test_inverse_asymmetric_image() {
    let center = DVec2::new(960.0, 540.0);
    let k = 5e-9;

    let mut ref_points = Vec::new();
    let mut residuals = Vec::new();
    for y in (0..=1080).step_by(90) {
        for x in (0..=1920).step_by(120) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            residuals.push(d * k * d.length_squared());
        }
    }
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };
    let mut sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();
    let max_err = sip.compute_inverse(1920, 1080);
    assert!(
        max_err < 0.1,
        "asymmetric image roundtrip error {:.6} >= 0.1",
        max_err
    );

    // Spot-check corners
    for &p in &[
        DVec2::new(0.0, 0.0),
        DVec2::new(1920.0, 0.0),
        DVec2::new(0.0, 1080.0),
        DVec2::new(1920.0, 1080.0),
    ] {
        let corrected = sip.correct(p);
        let roundtrip = sip.inverse_correct(corrected);
        let err = (roundtrip - p).length();
        assert!(err < 0.1, "corner roundtrip error {:.6} at {:?}", err, p);
    }
}
