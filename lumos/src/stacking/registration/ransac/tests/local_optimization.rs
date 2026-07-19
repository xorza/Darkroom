use crate::stacking::registration::ransac::tests::*;

#[test]
fn test_lo_ransac_converges_to_exact_solution() {
    let ref_points = make_grid(4, 2, 10.0);
    let known = Transform::translation(DVec2::new(5.0, 3.0));
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        local_optimization: true,
        lo_iterations: 10,
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 8);
    let t = result.transform.translation_components();
    assert!((t.x - 5.0).abs() < 0.01);
    assert!((t.y - 3.0).abs() < 0.01);
}

#[test]
fn test_lo_ransac_vs_standard_with_noisy_data() {
    // With noisy data, LO should find at least as many inliers
    let ref_points = make_grid(5, 4, 20.0);
    let known = Transform::similarity(DVec2::new(10.0, -5.0), PI / 8.0, 1.1);
    let mut target_points = apply_all(&known, &ref_points);

    // Add noise to some points
    target_points[5].x += 0.5;
    target_points[10].y -= 0.3;

    let matches = make_matches(ref_points.len());

    let result_with_lo = estimator_with_max_sigma(
        0.33,
        RansacConfig {
            seed: Some(123),
            local_optimization: true,
            lo_iterations: 5,
            max_rotation: None,
            scale_range: None,
            ..Default::default()
        },
    )
    .estimate(
        &matches,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();

    let result_without_lo = estimator_with_max_sigma(
        0.33,
        RansacConfig {
            seed: Some(123),
            local_optimization: false,
            max_rotation: None,
            scale_range: None,
            ..Default::default()
        },
    )
    .estimate(
        &matches,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();

    assert!(
        result_with_lo.inliers.len() >= result_without_lo.inliers.len(),
        "LO-RANSAC: {} inliers, standard: {} inliers",
        result_with_lo.inliers.len(),
        result_without_lo.inliers.len()
    );
}

#[test]
fn test_final_refit_does_not_degrade_robust_score() {
    let ref_points: Vec<DVec2> = (0..10)
        .map(|index| DVec2::new(index as f64 * 10.0, 0.0))
        .collect();
    let mut target_points = ref_points.clone();
    for point in &mut target_points[8..] {
        point.x += 3.0;
    }

    let robust = Transform::translation(DVec2::ZERO);
    // The all-inlier translation refit is the mean displacement:
    // (8 × 0 + 2 × 3) / 10 = 0.6 pixels.
    let all_inlier_refit = Transform::translation(DVec2::new(0.6, 0.0));
    let scorer = MagsacScorer::new(1.0);
    let mut inliers = Vec::new();
    let robust_score = score_hypothesis(
        &ref_points,
        &target_points,
        &robust,
        &scorer,
        &mut inliers,
        f64::NEG_INFINITY,
    );
    assert_eq!(inliers, (0..10).collect::<Vec<_>>());
    let refit_score = score_hypothesis(
        &ref_points,
        &target_points,
        &all_inlier_refit,
        &scorer,
        &mut inliers,
        f64::NEG_INFINITY,
    );
    assert!(
        robust_score > refit_score,
        "robust score {robust_score} must beat refit score {refit_score}"
    );

    let estimator = make_estimator(RansacConfig {
        max_iterations: 1,
        local_optimization: false,
        ..Default::default()
    });
    let result = estimator
        .ransac_loop(
            &ref_points,
            &target_points,
            ref_points.len(),
            TransformType::Translation.min_points(),
            TransformType::Translation,
            |_, _, sample| {
                sample.clear();
                sample.push(0);
            },
        )
        .unwrap();

    assert_eq!(result.inliers, (0..10).collect::<Vec<_>>());
    assert_eq!(result.transform.translation_components(), DVec2::ZERO);
}
