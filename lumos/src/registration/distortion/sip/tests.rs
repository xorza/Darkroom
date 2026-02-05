use super::*;
use crate::registration::transform::Transform;

#[test]
fn test_fit_from_transform_barrel_distortion() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

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
fn test_fit_from_transform_pincushion() {
    let center = DVec2::new(512.0, 384.0);
    let k = -5e-8;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (50..=750).step_by(100) {
        for x in (50..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);

            let d = p - center;
            let r2 = d.length_squared();
            let distortion = d * k * r2;
            target_points.push(p + distortion);
        }
    }

    let transform = Transform::identity();

    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap();

    let residuals = sip.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let rms: f64 = (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len() as f64).sqrt();
    assert!(rms < 0.01, "RMS after SIP correction: {:.6}", rms);
}

#[test]
fn test_correct_applies_offset() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            let r2 = d.length_squared();
            target_points.push(p + d * k * r2);
        }
    }

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap();

    // correct() should move the point
    let test_p = DVec2::new(700.0, 300.0);
    let corrected = sip.correct(test_p);
    assert!(
        (corrected - test_p).length() > 1e-6,
        "Correction should move point"
    );
}

#[test]
fn test_max_correction() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            target_points.push(p + d * k * d.length_squared());
        }
    }

    let transform = Transform::identity();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
    };

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap();

    let max_corr = sip.max_correction(1000, 1000, 50.0);
    assert!(max_corr > 0.0, "Max correction should be positive");
}

#[test]
fn test_too_few_points() {
    let ref_points = vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 100.0)];
    let target_points = vec![DVec2::new(0.1, 0.0), DVec2::new(99.9, 100.0)];

    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::ZERO),
    };

    // 2 points < 3 terms for order 2 => should return None
    assert!(
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
            .is_none()
    );
}

#[test]
fn test_zero_distortion() {
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (0..=400).step_by(100) {
        for x in (0..=400).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            target_points.push(p); // No distortion
        }
    }

    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(DVec2::new(200.0, 200.0)),
    };

    let sip = SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config)
        .unwrap();

    // All corrections should be near zero
    for &p in &ref_points {
        let corrected = sip.correct(p);
        assert!(
            (corrected - p).length() < 1e-10,
            "Correction should be zero, got {:?}",
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
    };
    let ref_points = vec![DVec2::ZERO; 10];
    let target_points = vec![DVec2::ZERO; 10];
    let transform = Transform::identity();
    SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config);
}

#[test]
fn test_all_orders() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for y in (0..=1000).step_by(50) {
        for x in (0..=1000).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - center;
            target_points.push(p + d * k * d.length_squared());
        }
    }

    let transform = Transform::identity();

    for order in 2..=5 {
        let config = SipConfig {
            order,
            reference_point: Some(center),
        };

        let sip =
            SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config);
        assert!(sip.is_some(), "SIP order {} should fit successfully", order);
    }
}
