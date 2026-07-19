use crate::stacking::registration::ransac::tests::*;

#[test]
fn test_ransac_perfect_translation() {
    // 8 points with exact translation (5, -3) → all should be inliers
    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(7.0, 3.0),
        DVec2::new(2.0, 8.0),
        DVec2::new(9.0, 1.0),
    ];
    let target_points: Vec<DVec2> = ref_points
        .iter()
        .map(|p| *p + DVec2::new(15.0, -7.0))
        .collect();

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
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
    assert!((t.x - 15.0).abs() < 0.01);
    assert!((t.y - (-7.0)).abs() < 0.01);
}

#[test]
fn test_ransac_perfect_similarity() {
    let ref_points = make_grid(4, 2, 10.0); // 8 points
    let known = Transform::similarity(DVec2::new(5.0, -3.0), PI / 4.0, 1.2);
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

    assert_eq!(result.inliers.len(), 8);
    assert!((result.transform.rotation_angle() - PI / 4.0).abs() < 0.01);
    assert!((result.transform.scale_factor() - 1.2).abs() < 0.01);
}

#[test]
fn test_ransac_with_outliers() {
    // 8 inliers + 2 outliers
    let mut ref_points: Vec<DVec2> = make_grid(4, 2, 10.0);
    ref_points.push(DVec2::new(100.0, 100.0));
    ref_points.push(DVec2::new(200.0, 200.0));

    let known = Transform::translation(DVec2::new(5.0, 3.0));
    let mut target_points = apply_all(&known, &ref_points);

    // Make last two points outliers
    target_points[8] = DVec2::new(500.0, 500.0);
    target_points[9] = DVec2::new(600.0, 600.0);

    let estimator = estimator_with_max_sigma(
        0.33,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 8);
    assert!(!result.inliers.contains(&8));
    assert!(!result.inliers.contains(&9));

    let t = result.transform.translation_components();
    assert!((t.x - 5.0).abs() < 0.1);
    assert!((t.y - 3.0).abs() < 0.1);
}

#[test]
fn test_ransac_30_percent_outliers() {
    // 10 inliers + 4 outliers
    let ref_points: Vec<DVec2> = make_grid(5, 2, 10.0)
        .into_iter()
        .chain(vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(150.0, 50.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(250.0, 150.0),
        ])
        .collect();

    let known = Transform::translation(DVec2::new(5.0, 3.0));
    let mut target_points = apply_all(&known, &ref_points);

    // Make last 4 outliers
    target_points[10] = DVec2::new(500.0, 500.0);
    target_points[11] = DVec2::new(600.0, 300.0);
    target_points[12] = DVec2::new(700.0, 700.0);
    target_points[13] = DVec2::new(800.0, 400.0);

    let estimator = estimator_with_max_sigma(
        0.33,
        RansacConfig {
            seed: Some(42),
            local_optimization: true,
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 10);
    for outlier_idx in 10..14 {
        assert!(
            !result.inliers.contains(&outlier_idx),
            "Outlier {} should not be in inliers",
            outlier_idx
        );
    }
}

#[test]
fn test_ransac_insufficient_points() {
    // Similarity needs min 2 points
    let ref_points = [DVec2::new(0.0, 0.0)];
    let target_points = [DVec2::new(1.0, 1.0)];

    let estimator = make_estimator(RansacConfig::default());
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(result.is_none());
}

#[test]
fn test_ransac_empty_matches() {
    let matches: Vec<PointMatch> = vec![];
    let ransac = make_estimator(RansacConfig::default());
    let result = ransac.estimate(&matches, &[], &[], TransformType::Translation);
    assert!(result.is_none());
}

#[test]
fn test_ransac_minimum_points_for_translation() {
    // Translation needs 1 point minimum (min_points), but RANSAC samples 1 point.
    // With 2 points, sampling is guaranteed to succeed.
    let ref_points = [DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0)];
    let target_points = [DVec2::new(10.0, 10.0), DVec2::new(110.0, 10.0)];

    let estimator = estimator_with_max_sigma(
        0.33,
        RansacConfig {
            max_iterations: 100,
            min_inlier_ratio: 0.5,
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 2);
    let t = result.transform.translation_components();
    assert!((t.x - 10.0).abs() < 0.01);
    assert!((t.y - 10.0).abs() < 0.01);
}

#[test]
fn test_ransac_deterministic_with_seed() {
    let ref_points = make_grid(3, 2, 10.0); // 6 points
    let target_points: Vec<DVec2> = ref_points
        .iter()
        .map(|p| *p + DVec2::new(5.0, 3.0))
        .collect();

    let config = RansacConfig {
        seed: Some(12345),
        ..Default::default()
    };

    let estimator1 = make_estimator(config.clone());
    let result1 = estimate_uniform(
        &estimator1,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    let estimator2 = make_estimator(config);
    let result2 = estimate_uniform(
        &estimator2,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result1.inliers, result2.inliers);
    assert_eq!(result1.iterations, result2.iterations);

    let t1 = result1.transform.translation_components();
    let t2 = result2.transform.translation_components();
    assert!((t1.x - t2.x).abs() < TOL);
    assert!((t1.y - t2.y).abs() < TOL);
}

#[test]
fn test_ransac_early_termination() {
    // All 50 perfect inliers. With 100% inlier ratio and conf=0.999,
    // adaptive_iterations(1.0, 1, 0.999) = 1, so it should terminate very early.
    let ref_points = make_grid(10, 5, 10.0);
    let transform = Transform::translation(DVec2::new(7.0, 3.0));
    let target_points = apply_all(&transform, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.33,
        RansacConfig {
            max_iterations: 10000,
            confidence: 0.999,
            ..Default::default()
        },
    );

    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    // All 50 points should be inliers
    assert_eq!(result.inliers.len(), 50);
    // Should have terminated early (much fewer than 10000 iterations)
    assert!(
        result.iterations < 100,
        "Expected early termination, got {} iterations",
        result.iterations
    );

    let t = result.transform.translation_components();
    assert!((t.x - 7.0).abs() < 0.01);
    assert!((t.y - 3.0).abs() < 0.01);
}

#[test]
fn test_ransac_100_percent_inliers() {
    let ref_points = make_grid(4, 2, 50.0); // 8 points
    let transform = Transform::similarity(DVec2::new(10.0, -5.0), 0.2, 1.1);
    let target_points = apply_all(&transform, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.33,
        RansacConfig {
            max_iterations: 100,
            max_rotation: None,
            scale_range: None,
            ..Default::default()
        },
    );

    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 8);
}

#[test]
fn test_ransac_affine() {
    let ref_points = make_grid(4, 2, 25.0);
    let known = Transform::affine([1.1, 0.2, 10.0, -0.1, 0.95, 5.0]);
    let target_points = apply_all(&known, &ref_points);

    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        ..Default::default()
    });
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Affine,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 8);

    // Verify the transform is accurate
    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = result.transform.apply(rp);
        assert!(
            (pp - tp).length() < 0.1,
            "Error at ({},{}): {:.4}",
            rp.x,
            rp.y,
            (pp - tp).length()
        );
    }
}

#[test]
fn test_ransac_affine_with_shear() {
    // Shear: x' = x + 0.3*y + 10, y' = 0.1*x + y - 5
    let ref_points = make_grid(5, 4, 50.0);
    let known = Transform::affine([1.0, 0.3, 10.0, 0.1, 1.0, -5.0]);
    let target_points = apply_all(&known, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.17,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Affine,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 20);

    for i in 0..ref_points.len() {
        let error = (result.transform.apply(ref_points[i]) - target_points[i]).length();
        assert!(error < 0.1, "Error {} at point {}", error, i);
    }
}

#[test]
fn test_ransac_homography() {
    let ref_points = make_grid(4, 2, 25.0);
    let known = Transform::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
    let target_points = apply_all(&known, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.33,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Homography,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 8);
}

#[test]
fn test_ransac_homography_near_affine() {
    // Homography with tiny perspective components
    let ref_points = make_grid(4, 2, 25.0);
    let known = Transform::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 1e-8, 1e-8]);
    let target_points = apply_all(&known, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.17,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Homography,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 8);

    for &i in &result.inliers {
        let error = (result.transform.apply(ref_points[i]) - target_points[i]).length();
        assert!(error < 0.5, "Error {} at point {}", error, i);
    }
}

#[test]
fn test_ransac_large_coordinates() {
    // Points at ~2000 range (typical high-res images)
    let ref_points: Vec<DVec2> = (0..10)
        .map(|i| {
            DVec2::new(
                2000.0 + (i % 5) as f64 * 100.0,
                1500.0 + (i / 5) as f64 * 100.0,
            )
        })
        .collect();

    let known = Transform::similarity(DVec2::new(50.0, -30.0), PI / 16.0, 1.05);
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

    assert_eq!(result.inliers.len(), 10);
    for i in 0..ref_points.len() {
        let error = (result.transform.apply(ref_points[i]) - target_points[i]).length();
        assert!(error < 0.1, "Error {} at point {}", error, i);
    }
}

#[test]
fn test_ransac_extreme_scale_coordinates() {
    // Points at 1e6 scale
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 1e6, (i / 5) as f64 * 1e6))
        .collect();

    let known = Transform::translation(DVec2::new(5000.0, -3000.0));
    let target_points = apply_all(&known, &ref_points);

    let estimator = estimator_with_max_sigma(
        33.0,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 20);
    let t = result.transform.translation_components();
    assert!((t.x - 5000.0).abs() < 1.0);
    assert!((t.y - (-3000.0)).abs() < 1.0);
}

#[test]
fn test_ransac_small_translation() {
    // Small sub-pixel translation
    let ref_points = make_grid(5, 4, 10.0);
    let known = Transform::translation(DVec2::new(0.5, -0.3));
    let target_points = apply_all(&known, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.033,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 20);
    let t = result.transform.translation_components();
    assert!((t.x - 0.5).abs() < 0.01);
    assert!((t.y - (-0.3)).abs() < 0.01);
}

#[test]
fn test_ransac_mixed_scale_coordinates() {
    // Points spanning very different scales
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1000.0, 1000.0),
        DVec2::new(1001.0, 1000.0),
        DVec2::new(1000.0, 1001.0),
        DVec2::new(5000.0, 0.0),
        DVec2::new(0.0, 5000.0),
        DVec2::new(2500.0, 2500.0),
        DVec2::new(100.0, 100.0),
    ];

    let known = Transform::translation(DVec2::new(10.0, -5.0));
    let target_points = apply_all(&known, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.17,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 10);
    let t = result.transform.translation_components();
    assert!((t.x - 10.0).abs() < 0.01);
    assert!((t.y - (-5.0)).abs() < 0.01);
}

#[test]
fn test_similarity_very_small_rotation() {
    let ref_points = make_grid(5, 4, 100.0);
    let tiny_angle = 0.001; // ~0.057 degrees
    let known = Transform::similarity(DVec2::new(5.0, 3.0), tiny_angle, 1.0);
    let target_points = apply_all(&known, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.17,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 20);
    assert!(
        (result.transform.rotation_angle() - tiny_angle).abs() < 0.0005,
        "Expected angle ~{}, got {}",
        tiny_angle,
        result.transform.rotation_angle()
    );
}

#[test]
fn test_similarity_near_unity_scale() {
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| {
            DVec2::new(
                100.0 + (i % 5) as f64 * 100.0,
                100.0 + (i / 5) as f64 * 100.0,
            )
        })
        .collect();

    let tiny_scale = 1.0001;
    let known = Transform::similarity(DVec2::new(2.0, -1.0), 0.0, tiny_scale);
    let target_points = apply_all(&known, &ref_points);

    let estimator = estimator_with_max_sigma(
        0.17,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();

    assert_eq!(result.inliers.len(), 20);
    assert!(
        (result.transform.scale_factor() - tiny_scale).abs() < 0.0001,
        "Expected scale ~{}, got {}",
        tiny_scale,
        result.transform.scale_factor()
    );
}
