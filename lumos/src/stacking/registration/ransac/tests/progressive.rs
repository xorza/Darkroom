use crate::stacking::registration::ransac::tests::*;

#[test]
fn test_progressive_ransac_basic() {
    let ref_points = make_grid(5, 4, 20.0);
    let offset = DVec2::new(15.0, -8.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|p| *p + offset).collect();

    let confidences = vec![0.9; 20];
    let estimator = make_estimator(RansacConfig {
        seed: Some(42),
        ..Default::default()
    });

    let result = estimator
        .estimate(
            &make_matches_with_confidence(&confidences),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    let est = result.transform.translation_components();
    assert!((est.x - 15.0).abs() < 0.01);
    assert!((est.y - (-8.0)).abs() < 0.01);
    assert_eq!(result.inliers.len(), 20);
}

#[test]
fn test_progressive_ransac_outlier_rejection() {
    // 15 inliers (high confidence) + 5 outliers (low confidence)
    let mut ref_points = make_grid(5, 3, 20.0);
    let offset = DVec2::new(10.0, 5.0);
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|p| *p + offset).collect();

    // 5 outliers
    for i in 0..5 {
        ref_points.push(DVec2::new(100.0 + i as f64 * 10.0, 100.0));
        target_points.push(DVec2::new(200.0 + i as f64 * 5.0, 50.0));
    }

    let mut confidences = vec![0.9; 15];
    confidences.extend(vec![0.1; 5]);

    let estimator = make_estimator(RansacConfig {
        seed: Some(123),
        max_iterations: 500,
        ..Default::default()
    });

    let result = estimator
        .estimate(
            &make_matches_with_confidence(&confidences),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    let est = result.transform.translation_components();
    assert!((est.x - 10.0).abs() < 0.5);
    assert!((est.y - 5.0).abs() < 0.5);

    // Should find all 15 inliers
    assert_eq!(result.inliers.len(), 15);
}

#[test]
fn test_progressive_ransac_uses_weights() {
    // First 5 points are outliers with low confidence,
    // remaining 15 are inliers with high confidence
    let ref_points = make_grid(5, 4, 10.0);
    let transform = Transform::translation(DVec2::new(5.0, 3.0));
    let mut target_points = apply_all(&transform, &ref_points);

    // Corrupt first 5 points
    for point in target_points.iter_mut().take(5) {
        point.x += 50.0;
        point.y += 50.0;
    }

    let confidences: Vec<f64> = (0..20).map(|i| if i < 5 { 0.1 } else { 0.9 }).collect();

    let estimator = estimator_with_max_sigma(
        0.67,
        RansacConfig {
            max_iterations: 200,
            ..Default::default()
        },
    );

    let result = estimator
        .estimate(
            &make_matches_with_confidence(&confidences),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    // Should find the 15 good points as inliers
    assert_eq!(
        result.inliers.len(),
        15,
        "Expected 15 inliers, got {}",
        result.inliers.len()
    );

    // Verify outliers not in inliers
    for idx in 0..5 {
        assert!(!result.inliers.contains(&idx), "Outlier {} in inliers", idx);
    }
}

#[test]
fn test_progressive_ransac_finds_solution_faster() {
    // Progressive RANSAC should converge faster than max_iterations
    let ref_points = make_grid(10, 5, 10.0);
    let angle = PI / 12.0;
    let scale = 1.2;
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle, scale);
    let target_points = apply_all(&known, &ref_points);

    let confidences: Vec<f64> = (0..50)
        .map(|i| {
            let x = (i % 10) as f64;
            let y = (i / 10) as f64;
            let dist = ((x - 4.5).powi(2) + (y - 2.0).powi(2)).sqrt();
            1.0 / (1.0 + dist * 0.1)
        })
        .collect();

    let estimator = make_estimator(RansacConfig {
        seed: Some(999),
        max_iterations: 200,
        max_rotation: None,
        scale_range: None,
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

    assert_eq!(result.inliers.len(), 50);
    assert!((result.transform.rotation_angle() - angle).abs() < 0.01);
    assert!((result.transform.scale_factor() - scale).abs() < 0.01);
}

#[test]
fn test_estimate_with_varying_confidence() {
    let ref_stars = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 100.0),
        DVec2::new(100.0, 200.0),
        DVec2::new(200.0, 200.0),
        DVec2::new(150.0, 150.0),
    ];

    let offset = DVec2::new(50.0, -30.0);
    let target_stars: Vec<DVec2> = ref_stars.iter().map(|p| *p + offset).collect();

    let matches: Vec<PointMatch> = (0..ref_stars.len())
        .map(|i| PointMatch {
            ref_idx: i,
            target_idx: i,
            votes: 10 - i,
            confidence: 1.0 - (i as f64 * 0.1),
        })
        .collect();

    let ransac = make_estimator(RansacConfig {
        seed: Some(42),
        ..Default::default()
    });
    let result = ransac
        .estimate(
            &matches,
            &ref_stars,
            &target_stars,
            TransformType::Translation,
        )
        .unwrap();

    let est = result.transform.translation_components();
    assert!((est.x - 50.0).abs() < 0.1);
    assert!((est.y - (-30.0)).abs() < 0.1);
}

#[test]
fn test_estimate_rejects_outlier_with_low_confidence() {
    let ref_stars = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 100.0),
        DVec2::new(100.0, 200.0),
        DVec2::new(200.0, 200.0),
        DVec2::new(150.0, 150.0), // will be outlier
    ];

    let offset = DVec2::new(50.0, -30.0);
    let mut target_stars: Vec<DVec2> = ref_stars.iter().map(|p| *p + offset).collect();
    target_stars[4] = DVec2::new(1000.0, 1000.0); // outlier

    let matches: Vec<PointMatch> = (0..5)
        .map(|i| PointMatch {
            ref_idx: i,
            target_idx: i,
            votes: if i == 4 { 1 } else { 10 },
            confidence: if i == 4 { 0.01 } else { 0.9 },
        })
        .collect();

    let ransac = estimator_with_max_sigma(
        1.67,
        RansacConfig {
            seed: Some(42),
            ..Default::default()
        },
    );
    let result = ransac
        .estimate(
            &matches,
            &ref_stars,
            &target_stars,
            TransformType::Translation,
        )
        .unwrap();

    assert!(!result.inliers.contains(&4), "Outlier should not be inlier");
    assert_eq!(result.inliers.len(), 4);

    let est = result.transform.translation_components();
    assert!((est.x - 50.0).abs() < 0.5);
    assert!((est.y - (-30.0)).abs() < 0.5);
}
