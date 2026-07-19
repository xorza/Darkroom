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
