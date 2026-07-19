use crate::stacking::registration::distortion::sip::tests::*;

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
        config.validate().unwrap();
    }
}

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

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let expected_norm_scale = avg_distance(&ref_points, center);
    assert!(
        (sip.norm_scale - expected_norm_scale).abs() < 1e-10,
        "norm_scale: got {:.6}, expected {:.6}",
        sip.norm_scale,
        expected_norm_scale
    );
}

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

    let result = fit_sip(&ref_points, &target_points, &transform, &config);

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

    let result = fit_sip(&ref_points, &target_points, &transform, &config);

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

    let result = fit_sip(&ref_points, &target_points, &transform, &config);

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
    let r1 = fit_sip(&ref_1, &tgt_1, &transform, &config);
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
    let r2 = fit_sip(&ref_2, &tgt_2, &transform, &config);
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
    let r3 = fit_sip(&ref_2, &tgt_2, &transform, &config_no_clip);
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
    let r = fit_sip(&ref_pts, &tgt_pts, &transform, &config);
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
    let r2 = fit_sip(&ref_pts2, &tgt_pts2, &transform, &config);
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

    let r3 = fit_sip(&ref_points, &target_points, &transform, &config_3);
    let r4 = fit_sip(&ref_points, &target_points, &transform, &config_4);

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

    let result = fit_sip(&ref_points, &target_points, &transform, &config);

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

    let r_clip = fit_sip(&ref_points, &target_points, &transform, &config_clip);
    let r_no_clip = fit_sip(&ref_points, &target_points, &transform, &config_no_clip);

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
