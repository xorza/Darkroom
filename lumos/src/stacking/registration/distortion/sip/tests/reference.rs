use crate::stacking::registration::distortion::sip::tests::*;

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

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

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

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

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
