use crate::stacking::registration::ransac::tests::*;

#[test]
fn test_plausibility_rejects_large_rotation() {
    let ref_points = make_grid(5, 4, 50.0);
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 30.0_f64.to_radians(), 1.0);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: None,
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should reject 30deg rotation with 10deg limit"
    );
}

#[test]
fn test_plausibility_rejects_negative_rotation() {
    let ref_points = make_grid(5, 4, 50.0);
    let known = Transform::similarity(DVec2::new(5.0, -3.0), -30.0_f64.to_radians(), 1.0);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: None,
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should reject -30deg rotation with 10deg limit"
    );
}

#[test]
fn test_plausibility_rejects_large_scale() {
    let ref_points = make_grid(5, 4, 50.0);
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 0.0, 2.0);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: None,
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should reject scale 2.0 with (0.8, 1.2) range"
    );
}

#[test]
fn test_plausibility_rejects_small_scale() {
    let ref_points = make_grid(5, 4, 50.0);
    let known = Transform::similarity(DVec2::new(0.0, 0.0), 0.0, 0.5);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: None,
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should reject scale 0.5 with (0.8, 1.2) range"
    );
}

#[test]
fn test_plausibility_accepts_within_bounds() {
    let ref_points = make_grid(5, 4, 50.0);
    let angle = 5.0_f64.to_radians();
    let scale = 1.1;
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle, scale);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();

    assert!(approx_eq(result.transform.rotation_angle(), angle, 0.02));
    assert!(approx_eq(result.transform.scale_factor(), scale, 0.02));
}

#[test]
fn test_plausibility_disabled_accepts_everything() {
    let ref_points = make_grid(5, 4, 50.0);
    let known = Transform::similarity(DVec2::new(5.0, -3.0), PI / 4.0, 2.0);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 20);
}

#[test]
fn test_plausibility_rotation_boundary() {
    let ref_points = make_grid(5, 4, 50.0);
    let max_rotation = 10.0_f64.to_radians();

    // 9.9 degrees -- should pass
    let angle_pass = 9.9_f64.to_radians();
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle_pass, 1.0);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: Some(max_rotation),
        scale_range: None,
        ..Default::default()
    });
    assert!(
        estimate_uniform(
            &estimator,
            &ref_points,
            &target_points,
            TransformType::Similarity
        )
        .is_some(),
        "9.9deg should pass 10deg limit"
    );

    // 10.5 degrees -- should fail
    let angle_fail = 10.5_f64.to_radians();
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle_fail, 1.0);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: Some(max_rotation),
        scale_range: None,
        ..Default::default()
    });
    assert!(
        estimate_uniform(
            &estimator,
            &ref_points,
            &target_points,
            TransformType::Similarity
        )
        .is_none(),
        "10.5deg should fail 10deg limit"
    );
}

#[test]
fn test_plausibility_scale_boundary() {
    let ref_points = make_grid(5, 4, 50.0);

    // 1.15 scale -- should pass (0.8, 1.2)
    let known = Transform::similarity(DVec2::new(0.0, 0.0), 0.0, 1.15);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: None,
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    });
    assert!(
        estimate_uniform(
            &estimator,
            &ref_points,
            &target_points,
            TransformType::Similarity
        )
        .is_some(),
        "1.15 scale should pass (0.8, 1.2)"
    );

    // 1.25 scale -- should fail (0.8, 1.2)
    let known = Transform::similarity(DVec2::new(0.0, 0.0), 0.0, 1.25);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: None,
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    });
    assert!(
        estimate_uniform(
            &estimator,
            &ref_points,
            &target_points,
            TransformType::Similarity
        )
        .is_none(),
        "1.25 scale should fail (0.8, 1.2)"
    );
}

#[test]
fn test_plausibility_combined_rotation_and_scale() {
    let ref_points = make_grid(5, 4, 50.0);
    let config_base = RansacConfig {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };

    // Rotation OK (5deg) but scale too large (1.5) -- should fail
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 5.0_f64.to_radians(), 1.5);
    let target_points = apply_all(&known, &ref_points);
    let estimator = make_estimator(config_base.clone());
    assert!(
        estimate_uniform(
            &estimator,
            &ref_points,
            &target_points,
            TransformType::Similarity
        )
        .is_none(),
        "Should fail: rotation OK, scale out of range"
    );

    // Scale OK (1.1) but rotation too large (20deg) -- should fail
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 20.0_f64.to_radians(), 1.1);
    let target_points = apply_all(&known, &ref_points);
    let estimator = make_estimator(config_base.clone());
    assert!(
        estimate_uniform(
            &estimator,
            &ref_points,
            &target_points,
            TransformType::Similarity
        )
        .is_none(),
        "Should fail: scale OK, rotation out of range"
    );

    // Both within range -- should pass
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 5.0_f64.to_radians(), 1.1);
    let target_points = apply_all(&known, &ref_points);
    let estimator = make_estimator(config_base);
    assert!(
        estimate_uniform(
            &estimator,
            &ref_points,
            &target_points,
            TransformType::Similarity
        )
        .is_some(),
        "Should pass when both within bounds"
    );
}

#[test]
fn test_plausibility_translation_unaffected() {
    // Pure translation should always pass tight plausibility checks
    let ref_points = make_grid(5, 4, 50.0);
    let target_points: Vec<DVec2> = ref_points
        .iter()
        .map(|p| *p + DVec2::new(100.0, -50.0))
        .collect();

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: Some(1.0_f64.to_radians()),
        scale_range: Some((0.99, 1.01)),
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 20);
}

#[test]
fn test_plausibility_progressive_ransac_respects_checks() {
    // Progressive RANSAC should also reject implausible transforms
    let ref_points = make_grid(5, 4, 50.0);
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 30.0_f64.to_radians(), 1.0);
    let target_points = apply_all(&known, &ref_points);
    let confidences = vec![0.9; 20];

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        ..Default::default()
    });
    let result = estimator.estimate(
        &make_matches_with_confidence(&confidences),
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Progressive RANSAC should reject 30deg rotation"
    );

    // But should accept 5deg
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 5.0_f64.to_radians(), 1.05);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    });
    let result = estimator
        .estimate(
            &make_matches_with_confidence(&confidences),
            &ref_points,
            &target_points,
            TransformType::Similarity,
        )
        .unwrap();

    assert_eq!(result.inliers.len(), 20);
}

#[test]
fn test_plausibility_with_outliers_filters_bad_hypotheses() {
    // 15 inliers + 2 outliers that would produce wild transforms if sampled
    let ref_points: Vec<DVec2> = make_grid(5, 3, 50.0);
    let offset = DVec2::new(10.0, -5.0);
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|p| *p + offset).collect();

    let mut ref_with_outliers = ref_points.clone();
    ref_with_outliers.push(DVec2::new(100.0, 100.0));
    ref_with_outliers.push(DVec2::new(200.0, 50.0));
    target_points.push(DVec2::new(500.0, -300.0));
    target_points.push(DVec2::new(-100.0, 800.0));

    let estimator = estimator_with_max_sigma(
        0.67,
        RansacConfig {
            seed: Some(42),
            max_rotation: Some(5.0_f64.to_radians()),
            scale_range: Some((0.9, 1.1)),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_with_outliers,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 15);

    let t = result.transform.translation_components();
    assert!((t.x - 10.0).abs() < 0.1);
    assert!((t.y - (-5.0)).abs() < 0.1);
}
