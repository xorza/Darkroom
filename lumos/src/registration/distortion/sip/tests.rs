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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
    };

    // 2 points < 3*3=9 minimum for order 2 => should return None
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
        ..Default::default()
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
            ..Default::default()
        };

        let sip =
            SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config);
        assert!(sip.is_some(), "SIP order {} should fit successfully", order);
    }
}

#[test]
fn test_reference_point_none_uses_centroid() {
    // When reference_point is None, the centroid of input points should be used
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    // Create a grid centered around (500, 500)
    for y in (200..=800).step_by(100) {
        for x in (200..=800).step_by(100) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            // Add small radial distortion from center
            let center = DVec2::new(500.0, 500.0);
            let d = p - center;
            let k = 1e-7;
            target_points.push(p + d * k * d.length_squared());
        }
    }

    let transform = Transform::identity();

    // Fit with None reference point (should use centroid)
    let config_none = SipConfig {
        order: 3,
        reference_point: None,
        ..Default::default()
    };
    let sip_none =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_none)
            .unwrap();

    // Verify correction works well (centroid ≈ center of grid ≈ 500,500)
    let residuals = sip_none.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let rms: f64 = (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len() as f64).sqrt();
    assert!(
        rms < 0.1,
        "RMS with centroid reference should be small: {:.6}",
        rms
    );
}

#[test]
fn test_reference_point_crpix_vs_centroid() {
    // FITS CRPIX is typically image center, which may differ from point centroid.
    // When distortion is radial from image center, using CRPIX produces better fits
    // than using the centroid of clustered points.
    let image_center = DVec2::new(512.0, 384.0); // Typical image center
    let k = 1e-7;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    // Create points clustered in one quadrant (centroid ≠ image center)
    for y in (100..=350).step_by(50) {
        for x in (100..=450).step_by(50) {
            let p = DVec2::new(x as f64, y as f64);
            ref_points.push(p);
            let d = p - image_center;
            target_points.push(p + d * k * d.length_squared());
        }
    }

    let transform = Transform::identity();

    // Fit with CRPIX (image center) - the correct reference for this distortion
    let config_crpix = SipConfig {
        order: 3,
        reference_point: Some(image_center),
        ..Default::default()
    };
    let sip_crpix =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_crpix)
            .unwrap();

    // Fit with centroid - wrong reference point for this distortion pattern
    let config_centroid = SipConfig {
        order: 3,
        reference_point: None,
        ..Default::default()
    };
    let sip_centroid = SipPolynomial::fit_from_transform(
        &ref_points,
        &target_points,
        &transform,
        &config_centroid,
    )
    .unwrap();

    let residuals_crpix =
        sip_crpix.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let residuals_centroid =
        sip_centroid.compute_corrected_residuals(&ref_points, &target_points, &transform);

    let rms_crpix: f64 =
        (residuals_crpix.iter().map(|r| r * r).sum::<f64>() / residuals_crpix.len() as f64).sqrt();
    let rms_centroid: f64 = (residuals_centroid.iter().map(|r| r * r).sum::<f64>()
        / residuals_centroid.len() as f64)
        .sqrt();

    // CRPIX (correct reference) should produce much better fit
    assert!(
        rms_crpix < 0.1,
        "CRPIX reference RMS should be small: {:.6}",
        rms_crpix
    );

    // Using wrong reference point produces worse fit - this is expected!
    // This demonstrates why CRPIX support matters for FITS interoperability
    assert!(
        rms_crpix < rms_centroid,
        "CRPIX should fit better than centroid when distortion is radial from image center"
    );
}

// ============================================================================
// Sigma-clipping tests
// ============================================================================

/// Generate clean barrel-distortion point pairs for sigma-clipping tests.
fn barrel_distortion_points(
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

#[test]
fn test_sip_sigma_clipping_rejects_outliers() {
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (mut ref_points, mut target_points) = barrel_distortion_points(center, k, 100, 1000);

    let transform = Transform::identity();

    // Fit without outliers first to get baseline RMS
    let config_clipped = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default() // clip_sigma=3.0, clip_iterations=3
    };
    let sip_clean =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_clipped)
            .unwrap();
    let residuals_clean =
        sip_clean.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let rms_clean: f64 =
        (residuals_clean.iter().map(|r| r * r).sum::<f64>() / residuals_clean.len() as f64).sqrt();

    // Inject 3 gross outliers (shift target positions by 20 pixels)
    let n = ref_points.len();
    ref_points.push(DVec2::new(300.0, 300.0));
    target_points.push(DVec2::new(320.0, 280.0)); // +20 error
    ref_points.push(DVec2::new(700.0, 200.0));
    target_points.push(DVec2::new(685.0, 225.0)); // large error
    ref_points.push(DVec2::new(100.0, 800.0));
    target_points.push(DVec2::new(130.0, 810.0)); // large error

    // Fit WITHOUT clipping — outliers corrupt the polynomial
    let config_no_clip = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        ..Default::default()
    };
    let sip_no_clip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_no_clip)
            .unwrap();
    // Evaluate on clean points only (exclude injected outliers)
    let residuals_no_clip =
        sip_no_clip.compute_corrected_residuals(&ref_points[..n], &target_points[..n], &transform);
    let rms_no_clip: f64 = (residuals_no_clip.iter().map(|r| r * r).sum::<f64>()
        / residuals_no_clip.len() as f64)
        .sqrt();

    // Fit WITH clipping — outliers should be rejected
    let sip_clipped =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_clipped)
            .unwrap();
    let residuals_clipped =
        sip_clipped.compute_corrected_residuals(&ref_points[..n], &target_points[..n], &transform);
    let rms_clipped: f64 = (residuals_clipped.iter().map(|r| r * r).sum::<f64>()
        / residuals_clipped.len() as f64)
        .sqrt();

    // Clipped fit should be much better than unclipped on clean points
    assert!(
        rms_clipped < rms_no_clip,
        "Clipped RMS ({:.6}) should be less than unclipped RMS ({:.6})",
        rms_clipped,
        rms_no_clip
    );

    // Clipped fit should be close to the clean baseline
    assert!(
        rms_clipped < rms_clean * 2.0 + 0.01,
        "Clipped RMS ({:.6}) should be close to clean baseline ({:.6})",
        rms_clipped,
        rms_clean
    );
}

#[test]
fn test_sip_sigma_clipping_converges_early() {
    // Clean data with no outliers — clipping should not reject anything
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (ref_points, target_points) = barrel_distortion_points(center, k, 100, 1000);

    let transform = Transform::identity();

    // Fit with clipping enabled
    let config_clipped = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..Default::default()
    };
    let sip_clipped =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_clipped)
            .unwrap();

    // Fit with clipping disabled
    let config_no_clip = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        ..Default::default()
    };
    let sip_no_clip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_no_clip)
            .unwrap();

    // Both should produce essentially identical results on clean data
    let residuals_clipped =
        sip_clipped.compute_corrected_residuals(&ref_points, &target_points, &transform);
    let residuals_no_clip =
        sip_no_clip.compute_corrected_residuals(&ref_points, &target_points, &transform);

    let rms_clipped: f64 = (residuals_clipped.iter().map(|r| r * r).sum::<f64>()
        / residuals_clipped.len() as f64)
        .sqrt();
    let rms_no_clip: f64 = (residuals_no_clip.iter().map(|r| r * r).sum::<f64>()
        / residuals_no_clip.len() as f64)
        .sqrt();

    assert!(
        (rms_clipped - rms_no_clip).abs() < 1e-10,
        "Clean data: clipped ({:.10}) and unclipped ({:.10}) should match",
        rms_clipped,
        rms_no_clip
    );
}

#[test]
fn test_sip_sigma_clipping_disabled_with_zero_iterations() {
    // clip_iterations=0 should produce identical results to the old behavior
    let center = DVec2::new(500.0, 500.0);
    let k = 1e-7;
    let (ref_points, target_points) = barrel_distortion_points(center, k, 100, 1000);

    let transform = Transform::identity();

    let config_zero = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        ..Default::default()
    };

    let config_default = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        clip_sigma: 3.0,
    };

    let sip_zero =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_zero)
            .unwrap();
    let sip_default =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &config_default)
            .unwrap();

    // Both should produce identical corrections at any point
    for &p in &[
        DVec2::new(0.0, 0.0),
        DVec2::new(500.0, 500.0),
        DVec2::new(1000.0, 1000.0),
        DVec2::new(200.0, 800.0),
    ] {
        let c_zero = sip_zero.correct(p);
        let c_default = sip_default.correct(p);
        assert!(
            (c_zero - c_default).length() < 1e-14,
            "Point {:?}: zero-iter ({:?}) != default ({:?})",
            p,
            c_zero,
            c_default
        );
    }
}
