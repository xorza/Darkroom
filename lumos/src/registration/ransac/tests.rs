//! Tests for RANSAC module.

use super::*;
use std::f64::consts::PI;

const EPSILON: f64 = 1e-6;

fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() < eps
}

#[test]
fn test_estimate_translation() {
    let ref_points = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|(x, y)| (x + 5.0, y - 3.0)).collect();

    let t = estimate_transform(&ref_points, &target_points, TransformType::Translation).unwrap();
    let (dx, dy) = t.translation_components();

    assert!(approx_eq(dx, 5.0, EPSILON));
    assert!(approx_eq(dy, -3.0, EPSILON));
}

#[test]
fn test_estimate_similarity() {
    let ref_points = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];

    // Apply known similarity transform
    let angle = PI / 6.0; // 30 degrees
    let scale = 1.5;
    let dx = 20.0;
    let dy = -10.0;

    let known = Transform::similarity(dx, dy, angle, scale);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Similarity).unwrap();

    // Check parameters
    assert!(approx_eq(estimated.rotation_angle(), angle, 0.01));
    assert!(approx_eq(estimated.scale_factor(), scale, 0.01));

    let (est_dx, est_dy) = estimated.translation_components();
    assert!(approx_eq(est_dx, dx, 0.1));
    assert!(approx_eq(est_dy, dy, 0.1));
}

#[test]
fn test_estimate_affine() {
    let ref_points = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Apply known affine transform (with shear)
    let known = Transform::affine([1.2, 0.3, 5.0, -0.1, 0.9, -3.0]);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let estimated = estimate_transform(&ref_points, &target_points, TransformType::Affine).unwrap();

    // Check that transform produces correct results
    for (&(rx, ry), &(tx, ty)) in ref_points.iter().zip(target_points.iter()) {
        let (px, py) = estimated.apply(rx, ry);
        assert!(approx_eq(px, tx, 0.01));
        assert!(approx_eq(py, ty, 0.01));
    }
}

#[test]
fn test_estimate_homography() {
    let ref_points = vec![
        (0.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (0.0, 100.0),
        (50.0, 50.0),
        (25.0, 75.0),
    ];

    // Apply known homography
    let known = Transform::homography([1.1, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Homography).unwrap();

    // Check that transform produces correct results
    for (&(rx, ry), &(tx, ty)) in ref_points.iter().zip(target_points.iter()) {
        let (px, py) = estimated.apply(rx, ry);
        assert!(
            approx_eq(px, tx, 0.5) && approx_eq(py, ty, 0.5),
            "Expected ({}, {}), got ({}, {})",
            tx,
            ty,
            px,
            py
        );
    }
}

#[test]
fn test_ransac_perfect_translation() {
    let ref_points = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
        (7.0, 3.0),
        (2.0, 8.0),
        (9.0, 1.0),
    ];
    let target_points: Vec<(f64, f64)> = ref_points
        .iter()
        .map(|(x, y)| (x + 15.0, y - 7.0))
        .collect();

    let config = RansacConfig {
        seed: Some(42),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator.estimate(&ref_points, &target_points, TransformType::Translation);

    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 8);

    let (dx, dy) = result.transform.translation_components();
    assert!(approx_eq(dx, 15.0, 0.1));
    assert!(approx_eq(dy, -7.0, 0.1));
}

#[test]
fn test_ransac_perfect_similarity() {
    let ref_points = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
        (7.0, 3.0),
        (2.0, 8.0),
        (9.0, 1.0),
    ];

    let known = Transform::similarity(5.0, -3.0, PI / 4.0, 1.2);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator.estimate(&ref_points, &target_points, TransformType::Similarity);

    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 8);
}

#[test]
fn test_ransac_with_outliers() {
    let ref_points = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
        (7.0, 3.0),
        (2.0, 8.0),
        (9.0, 1.0),
        (100.0, 100.0),
        (200.0, 200.0), // Outliers
    ];

    let known = Transform::translation(5.0, 3.0);
    let mut target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    // Make last two points outliers
    target_points[8] = (500.0, 500.0);
    target_points[9] = (600.0, 600.0);

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 1.0,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator.estimate(&ref_points, &target_points, TransformType::Translation);

    assert!(result.is_some());
    let result = result.unwrap();

    // Should have 8 inliers (excluding 2 outliers)
    assert_eq!(result.inliers.len(), 8);
    assert!(!result.inliers.contains(&8));
    assert!(!result.inliers.contains(&9));
}

#[test]
fn test_ransac_insufficient_points() {
    let ref_points = vec![(0.0, 0.0)];
    let target_points = vec![(1.0, 1.0)];

    let estimator = RansacEstimator::new(RansacConfig::default());
    let result = estimator.estimate(&ref_points, &target_points, TransformType::Similarity);

    assert!(result.is_none());
}

#[test]
fn test_adaptive_iterations() {
    // High inlier ratio should require few iterations
    let iters_high = adaptive_iterations(0.9, 2, 0.99);
    assert!(iters_high < 20);

    // Low inlier ratio should require more iterations than high ratio
    let iters_low = adaptive_iterations(0.3, 2, 0.99);
    assert!(iters_low > iters_high);
}

#[test]
fn test_centroid() {
    let points = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let (cx, cy) = centroid(&points);
    assert!(approx_eq(cx, 5.0, EPSILON));
    assert!(approx_eq(cy, 5.0, EPSILON));
}

#[test]
fn test_normalize_points() {
    let points = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let (normalized, _) = normalize_points(&points);

    // Check centroid is at origin
    let (cx, cy) = centroid(&normalized);
    assert!(approx_eq(cx, 0.0, EPSILON));
    assert!(approx_eq(cy, 0.0, EPSILON));

    // Check average distance is sqrt(2)
    let avg_dist: f64 = normalized
        .iter()
        .map(|(x, y)| (x * x + y * y).sqrt())
        .sum::<f64>()
        / normalized.len() as f64;
    assert!(approx_eq(avg_dist, std::f64::consts::SQRT_2, 0.01));
}

#[test]
fn test_ransac_affine() {
    let ref_points = vec![
        (0.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (0.0, 100.0),
        (50.0, 50.0),
        (25.0, 75.0),
        (75.0, 25.0),
        (33.0, 66.0),
    ];

    let known = Transform::affine([1.1, 0.2, 10.0, -0.1, 0.95, 5.0]);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator.estimate(&ref_points, &target_points, TransformType::Affine);

    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 8);
}

#[test]
fn test_ransac_homography() {
    let ref_points = vec![
        (0.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (0.0, 100.0),
        (50.0, 50.0),
        (25.0, 75.0),
        (75.0, 25.0),
        (33.0, 66.0),
    ];

    // Use a mild homography
    let known = Transform::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 1.0,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator.estimate(&ref_points, &target_points, TransformType::Homography);

    assert!(result.is_some());
}

// ============================================================================
// LO-RANSAC Tests
// ============================================================================

#[test]
fn test_lo_ransac_improves_inlier_count() {
    // Create points with some noise
    let ref_points: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 20.0;
            let y = (i / 5) as f64 * 20.0;
            (x, y)
        })
        .collect();

    let known = Transform::similarity(10.0, -5.0, PI / 8.0, 1.1);
    let mut target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    // Add some noise to make it harder
    target_points[5].0 += 0.5;
    target_points[10].1 -= 0.3;

    // Test with LO-RANSAC enabled
    let config_with_lo = RansacConfig {
        seed: Some(123),
        use_local_optimization: true,
        lo_max_iterations: 5,
        inlier_threshold: 1.0,
        ..Default::default()
    };
    let estimator_with = RansacEstimator::new(config_with_lo);
    let result_with = estimator_with
        .estimate(&ref_points, &target_points, TransformType::Similarity)
        .unwrap();

    // Test with LO-RANSAC disabled
    let config_without_lo = RansacConfig {
        seed: Some(123),
        use_local_optimization: false,
        inlier_threshold: 1.0,
        ..Default::default()
    };
    let estimator_without = RansacEstimator::new(config_without_lo);
    let result_without = estimator_without
        .estimate(&ref_points, &target_points, TransformType::Similarity)
        .unwrap();

    // LO-RANSAC should find at least as many inliers
    assert!(
        result_with.inliers.len() >= result_without.inliers.len(),
        "LO-RANSAC found {} inliers, standard found {}",
        result_with.inliers.len(),
        result_without.inliers.len()
    );
}

#[test]
fn test_lo_ransac_converges() {
    let ref_points = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
        (7.0, 3.0),
        (3.0, 7.0),
        (8.0, 8.0),
    ];

    let known = Transform::translation(5.0, 3.0);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        use_local_optimization: true,
        lo_max_iterations: 10,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Translation)
        .unwrap();

    // All points should be inliers
    assert_eq!(result.inliers.len(), 8);

    // Transform should be accurate
    let (dx, dy) = result.transform.translation_components();
    assert!(approx_eq(dx, 5.0, 0.1));
    assert!(approx_eq(dy, 3.0, 0.1));
}

#[test]
fn test_ransac_30_percent_outliers() {
    // 10 inliers, ~4 outliers (30%)
    let ref_points = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (20.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (20.0, 10.0),
        (0.0, 20.0),
        (10.0, 20.0),
        (20.0, 20.0),
        (5.0, 5.0),
        (100.0, 100.0), // outlier
        (150.0, 50.0),  // outlier
        (200.0, 200.0), // outlier
        (250.0, 150.0), // outlier
    ];

    let known = Transform::translation(5.0, 3.0);
    let mut target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    // Make outliers actually outliers
    target_points[10] = (500.0, 500.0);
    target_points[11] = (600.0, 300.0);
    target_points[12] = (700.0, 700.0);
    target_points[13] = (800.0, 400.0);

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 1.0,
        use_local_optimization: true,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Translation)
        .unwrap();

    // Should find 10 inliers
    assert_eq!(result.inliers.len(), 10);

    // Outliers should not be in inliers
    assert!(!result.inliers.contains(&10));
    assert!(!result.inliers.contains(&11));
    assert!(!result.inliers.contains(&12));
    assert!(!result.inliers.contains(&13));
}

#[test]
fn test_ransac_numerical_stability_large_coords() {
    // Points with large coordinates (typical for high-res images)
    let ref_points: Vec<(f64, f64)> = (0..10)
        .map(|i| {
            let x = 2000.0 + (i % 5) as f64 * 100.0;
            let y = 1500.0 + (i / 5) as f64 * 100.0;
            (x, y)
        })
        .collect();

    let known = Transform::similarity(50.0, -30.0, PI / 16.0, 1.05);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Similarity)
        .unwrap();

    // Should find all points as inliers
    assert_eq!(result.inliers.len(), 10);

    // Check accuracy
    for i in 0..ref_points.len() {
        let (rx, ry) = ref_points[i];
        let (tx, ty) = target_points[i];
        let (px, py) = result.transform.apply(rx, ry);
        let error = ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();
        assert!(
            error < 0.1,
            "Large coordinate error: {} at point {}",
            error,
            i
        );
    }
}

#[test]
fn test_ransac_deterministic_with_seed() {
    let ref_points = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|(x, y)| (x + 5.0, y + 3.0)).collect();

    let config = RansacConfig {
        seed: Some(12345),
        ..Default::default()
    };

    // Run twice with same seed
    let estimator1 = RansacEstimator::new(config.clone());
    let result1 = estimator1
        .estimate(&ref_points, &target_points, TransformType::Translation)
        .unwrap();

    let estimator2 = RansacEstimator::new(config);
    let result2 = estimator2
        .estimate(&ref_points, &target_points, TransformType::Translation)
        .unwrap();

    // Should get identical results
    assert_eq!(result1.inliers, result2.inliers);
    assert_eq!(result1.iterations, result2.iterations);

    let (dx1, dy1) = result1.transform.translation_components();
    let (dx2, dy2) = result2.transform.translation_components();
    assert!(approx_eq(dx1, dx2, EPSILON));
    assert!(approx_eq(dy1, dy2, EPSILON));
}

#[test]
fn test_progressive_ransac_basic() {
    // Create a simple translation scenario with confidence scores
    let ref_points: Vec<(f64, f64)> = (0..20)
        .map(|i| ((i % 5) as f64 * 20.0, (i / 5) as f64 * 20.0))
        .collect();

    let dx = 15.0;
    let dy = -8.0;
    let target_points: Vec<(f64, f64)> = ref_points.iter().map(|(x, y)| (x + dx, y + dy)).collect();

    // High confidence for all matches (perfect data)
    let confidences: Vec<f64> = vec![0.9; 20];

    let config = RansacConfig {
        seed: Some(42),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator
        .estimate_progressive(
            &ref_points,
            &target_points,
            &confidences,
            TransformType::Translation,
        )
        .unwrap();

    let (est_dx, est_dy) = result.transform.translation_components();
    assert!(approx_eq(est_dx, dx, 0.1));
    assert!(approx_eq(est_dy, dy, 0.1));
    assert_eq!(result.inliers.len(), 20);
}

#[test]
fn test_progressive_ransac_with_outliers() {
    // Create scenario where high-confidence matches are inliers
    // and low-confidence matches are outliers
    let mut ref_points: Vec<(f64, f64)> = (0..15)
        .map(|i| ((i % 5) as f64 * 20.0, (i / 5) as f64 * 20.0))
        .collect();

    let dx = 10.0;
    let dy = 5.0;
    let mut target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|(x, y)| (x + dx, y + dy)).collect();

    // Add 5 outliers with low confidence
    for i in 0..5 {
        ref_points.push((100.0 + i as f64 * 10.0, 100.0));
        target_points.push((200.0 + i as f64 * 5.0, 50.0)); // Wrong correspondence
    }

    // High confidence for inliers, low for outliers
    let mut confidences: Vec<f64> = vec![0.9; 15];
    confidences.extend(vec![0.1; 5]);

    let config = RansacConfig {
        seed: Some(123),
        max_iterations: 500,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator
        .estimate_progressive(
            &ref_points,
            &target_points,
            &confidences,
            TransformType::Translation,
        )
        .unwrap();

    let (est_dx, est_dy) = result.transform.translation_components();
    assert!(approx_eq(est_dx, dx, 0.5));
    assert!(approx_eq(est_dy, dy, 0.5));

    // Should find mostly inliers (the first 15 points)
    assert!(result.inliers.len() >= 12);
}

#[test]
fn test_progressive_ransac_finds_solution_faster() {
    // Progressive RANSAC should find a good solution in fewer iterations
    // when high-confidence matches are correct
    let ref_points: Vec<(f64, f64)> = (0..50)
        .map(|i| ((i % 10) as f64 * 10.0, (i / 10) as f64 * 10.0))
        .collect();

    let angle = PI / 12.0; // 15 degrees
    let scale = 1.2;
    let known = Transform::similarity(5.0, -3.0, angle, scale);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    // Varying confidence (higher for more central points)
    let confidences: Vec<f64> = (0..50)
        .map(|i| {
            let x = (i % 10) as f64;
            let y = (i / 10) as f64;
            let dist = ((x - 4.5).powi(2) + (y - 2.0).powi(2)).sqrt();
            1.0 / (1.0 + dist * 0.1)
        })
        .collect();

    let config = RansacConfig {
        seed: Some(999),
        max_iterations: 200,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator
        .estimate_progressive(
            &ref_points,
            &target_points,
            &confidences,
            TransformType::Similarity,
        )
        .unwrap();

    // Should find a good solution
    assert!(result.inlier_ratio > 0.9);
    assert!(approx_eq(result.transform.rotation_angle(), angle, 0.05));
    assert!(approx_eq(result.transform.scale_factor(), scale, 0.05));
}

// ============================================================================
// Numerical Stability Tests
// ============================================================================

#[test]
fn test_ransac_extreme_scale_1e6() {
    // Test with coordinates scaled by 1e6 (typical for high-res sensors)
    let ref_points: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 1e6;
            let y = (i / 5) as f64 * 1e6;
            (x, y)
        })
        .collect();

    let known = Transform::translation(5000.0, -3000.0);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 100.0, // Scaled threshold
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Translation)
        .unwrap();

    assert_eq!(result.inliers.len(), 20);
    let (dx, dy) = result.transform.translation_components();
    assert!(approx_eq(dx, 5000.0, 1.0));
    assert!(approx_eq(dy, -3000.0, 1.0));
}

#[test]
fn test_ransac_small_coordinates() {
    // Test with small but reasonable coordinates
    let ref_points: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 10.0;
            let y = (i / 5) as f64 * 10.0;
            (x, y)
        })
        .collect();

    let known = Transform::translation(0.5, -0.3);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 0.1,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Translation)
        .unwrap();

    assert_eq!(result.inliers.len(), 20);
    let (dx, dy) = result.transform.translation_components();
    assert!(approx_eq(dx, 0.5, 0.01));
    assert!(approx_eq(dy, -0.3, 0.01));
}

#[test]
fn test_ransac_mixed_scale_coordinates() {
    // Test with points at very different scales (some near origin, some far)
    let ref_points = vec![
        (0.0, 0.0),
        (1.0, 0.0),
        (0.0, 1.0),
        (1000.0, 1000.0),
        (1001.0, 1000.0),
        (1000.0, 1001.0),
        (5000.0, 0.0),
        (0.0, 5000.0),
        (2500.0, 2500.0),
        (100.0, 100.0),
    ];

    let known = Transform::translation(10.0, -5.0);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 0.5,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Translation)
        .unwrap();

    assert_eq!(result.inliers.len(), 10);
    let (dx, dy) = result.transform.translation_components();
    assert!(approx_eq(dx, 10.0, 0.1));
    assert!(approx_eq(dy, -5.0, 0.1));
}

#[test]
fn test_homography_near_affine() {
    // Homography with very small perspective components (nearly affine)
    let ref_points = vec![
        (0.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (0.0, 100.0),
        (50.0, 50.0),
        (25.0, 75.0),
        (75.0, 25.0),
        (33.0, 66.0),
    ];

    // Homography with tiny perspective components
    let known = Transform::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 1e-8, 1e-8]);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 0.5,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Homography)
        .unwrap();

    // Should still find all points as inliers
    assert!(result.inliers.len() >= 7);

    // Check transform accuracy
    for i in result.inliers.iter() {
        let (rx, ry) = ref_points[*i];
        let (tx, ty) = target_points[*i];
        let (px, py) = result.transform.apply(rx, ry);
        let error = ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();
        assert!(error < 1.0, "High error {} at point {}", error, i);
    }
}

#[test]
fn test_similarity_very_small_rotation() {
    // Very small rotation angle (< 0.1 degrees)
    let ref_points: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 100.0;
            let y = (i / 5) as f64 * 100.0;
            (x, y)
        })
        .collect();

    let tiny_angle = 0.001; // ~0.057 degrees
    let known = Transform::similarity(5.0, 3.0, tiny_angle, 1.0);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 0.5,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Similarity)
        .unwrap();

    assert_eq!(result.inliers.len(), 20);
    // Rotation angle should be recovered accurately
    assert!(
        (result.transform.rotation_angle() - tiny_angle).abs() < 0.0005,
        "Expected angle ~{}, got {}",
        tiny_angle,
        result.transform.rotation_angle()
    );
}

#[test]
fn test_similarity_near_unity_scale() {
    // Scale very close to 1.0 (typical for dithered exposures)
    let ref_points: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let x = 100.0 + (i % 5) as f64 * 100.0;
            let y = 100.0 + (i / 5) as f64 * 100.0;
            (x, y)
        })
        .collect();

    let tiny_scale = 1.0001; // 0.01% scale difference
    let known = Transform::similarity(2.0, -1.0, 0.0, tiny_scale);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 0.5,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Similarity)
        .unwrap();

    assert_eq!(result.inliers.len(), 20);
    assert!(
        (result.transform.scale_factor() - tiny_scale).abs() < 0.0001,
        "Expected scale ~{}, got {}",
        tiny_scale,
        result.transform.scale_factor()
    );
}

#[test]
fn test_affine_with_shear() {
    // Affine transform with significant shear
    let ref_points: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 50.0;
            let y = (i / 5) as f64 * 50.0;
            (x, y)
        })
        .collect();

    // Shear: x' = x + 0.3*y, y' = y + 0.1*x
    let known = Transform::affine([1.0, 0.3, 10.0, 0.1, 1.0, -5.0]);
    let target_points: Vec<(f64, f64)> =
        ref_points.iter().map(|&(x, y)| known.apply(x, y)).collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 0.5,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(&ref_points, &target_points, TransformType::Affine)
        .unwrap();

    assert_eq!(result.inliers.len(), 20);

    // Check all points transform correctly
    for i in 0..ref_points.len() {
        let (rx, ry) = ref_points[i];
        let (tx, ty) = target_points[i];
        let (px, py) = result.transform.apply(rx, ry);
        let error = ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();
        assert!(error < 0.1, "High error {} at point {}", error, i);
    }
}

#[test]
fn test_normalize_points_extreme_values() {
    // Test normalization with extreme coordinate values
    let points = vec![
        (1e10, 1e10),
        (1e10 + 1.0, 1e10),
        (1e10, 1e10 + 1.0),
        (1e10 + 1.0, 1e10 + 1.0),
    ];

    let (normalized, transform) = normalize_points(&points);

    // Check centroid is at origin
    let (cx, cy) = centroid(&normalized);
    assert!(cx.abs() < 1e-10, "Centroid x not at origin: {}", cx);
    assert!(cy.abs() < 1e-10, "Centroid y not at origin: {}", cy);

    // Transform should be invertible (denormalization should recover original)
    let inv = transform.inverse();
    for (orig, norm) in points.iter().zip(normalized.iter()) {
        let (rx, ry) = inv.apply(norm.0, norm.1);
        let dx: f64 = rx - orig.0;
        let dy: f64 = ry - orig.1;
        assert!(dx.abs() < 1e-5, "X mismatch: {} vs {}", rx, orig.0);
        assert!(dy.abs() < 1e-5, "Y mismatch: {} vs {}", ry, orig.1);
    }
}

// ============================================================================
// Convergence and edge case tests
// ============================================================================

/// Test RANSAC with 100% inliers (perfect match)
#[test]
fn test_ransac_100_percent_inliers() {
    // Perfect correspondences - all points are inliers
    let ref_points = vec![
        (0.0, 0.0),
        (100.0, 0.0),
        (0.0, 100.0),
        (100.0, 100.0),
        (50.0, 50.0),
        (25.0, 75.0),
        (75.0, 25.0),
        (33.0, 66.0),
    ];

    let transform = Transform::similarity(10.0, -5.0, 0.2, 1.1);
    let target_points: Vec<(f64, f64)> = ref_points
        .iter()
        .map(|&(x, y)| transform.apply(x, y))
        .collect();

    let config = RansacConfig {
        max_iterations: 100,
        inlier_threshold: 1.0,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator.estimate(&ref_points, &target_points, TransformType::Similarity);

    assert!(result.is_some(), "Should succeed with 100% inliers");
    let result = result.unwrap();

    // All points should be inliers
    assert_eq!(
        result.inliers.len(),
        ref_points.len(),
        "All {} points should be inliers, got {}",
        ref_points.len(),
        result.inliers.len()
    );
}

/// Test RANSAC with 0% inliers (pure noise) - should return None gracefully
#[test]
fn test_ransac_0_percent_inliers_pure_noise() {
    // Completely random points with no correspondence
    let ref_points = vec![
        (10.0, 20.0),
        (30.0, 40.0),
        (50.0, 60.0),
        (70.0, 80.0),
        (90.0, 100.0),
    ];

    // Target points are completely unrelated
    let target_points = vec![
        (500.0, 600.0),
        (700.0, 100.0),
        (200.0, 900.0),
        (800.0, 50.0),
        (150.0, 350.0),
    ];

    let config = RansacConfig {
        max_iterations: 100,
        inlier_threshold: 1.0,
        min_inlier_ratio: 0.8, // Require 80% inliers
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator.estimate(&ref_points, &target_points, TransformType::Similarity);

    // Should either return None or return a result with very few inliers
    if let Some(result) = result {
        // If it returns something, it should have low inlier count
        assert!(
            result.inliers.len() < 3,
            "Pure noise should have few inliers, got {}",
            result.inliers.len()
        );
    }
    // Returning None is also acceptable for pure noise
}

/// Test adaptive iteration count verification
#[test]
fn test_adaptive_iteration_count() {
    let config = RansacConfig {
        max_iterations: 10000,
        confidence: 0.999,
        ..Default::default()
    };

    // With 50% inliers and 2 points per model (similarity), we need:
    // k = log(1-0.999) / log(1 - 0.5^2) = log(0.001) / log(0.75) ≈ 24 iterations
    let adaptive_iters = adaptive_iterations(0.5, 2, config.confidence);
    assert!(
        adaptive_iters < 50,
        "50% inliers should need ~24 iterations, got {}",
        adaptive_iters
    );
    assert!(
        adaptive_iters > 10,
        "Should need at least 10 iterations, got {}",
        adaptive_iters
    );

    // With 90% inliers, we need very few iterations
    let high_inlier_iters = adaptive_iterations(0.9, 2, config.confidence);
    assert!(
        high_inlier_iters < 15,
        "90% inliers should need very few iterations, got {}",
        high_inlier_iters
    );

    // With 10% inliers, we need many iterations
    let low_inlier_iters = adaptive_iterations(0.1, 2, config.confidence);
    assert!(
        low_inlier_iters > 500,
        "10% inliers should need many iterations, got {}",
        low_inlier_iters
    );
}

/// Test that RANSAC early terminates when it finds a good model
#[test]
fn test_ransac_early_termination() {
    // All perfect inliers - should terminate early
    let ref_points: Vec<(f64, f64)> = (0..50).map(|i| (i as f64 * 10.0, i as f64 * 5.0)).collect();

    let transform = Transform::translation(7.0, 3.0);
    let target_points: Vec<(f64, f64)> = ref_points
        .iter()
        .map(|&(x, y)| transform.apply(x, y))
        .collect();

    let config = RansacConfig {
        max_iterations: 10000, // Very high, but should terminate early
        inlier_threshold: 1.0,
        confidence: 0.999,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator.estimate(&ref_points, &target_points, TransformType::Translation);

    assert!(result.is_some(), "Should find solution");
    let result = result.unwrap();

    // With perfect inliers, should find all 50 points as inliers
    assert!(
        result.inliers.len() >= 45,
        "Should find most points as inliers, got {}",
        result.inliers.len()
    );
}

/// Test homography estimation with nearly degenerate points
#[test]
fn test_homography_nearly_degenerate() {
    // Points that are nearly collinear but not quite
    let ref_points = vec![
        (0.0, 0.0),
        (100.0, 1.0),   // Nearly on x-axis
        (200.0, -1.0),  // Nearly on x-axis
        (300.0, 0.5),   // Nearly on x-axis
        (0.0, 100.0),   // This one breaks collinearity
        (100.0, 100.0), // This one breaks collinearity
    ];

    let transform = Transform::translation(10.0, 10.0);
    let target_points: Vec<(f64, f64)> = ref_points
        .iter()
        .map(|&(x, y)| transform.apply(x, y))
        .collect();

    let config = RansacConfig {
        max_iterations: 500,
        inlier_threshold: 2.0,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    // This may or may not succeed depending on which points are sampled
    // Key is it doesn't panic
    let result = estimator.estimate(&ref_points, &target_points, TransformType::Homography);

    // If it succeeds, check the transform is reasonable
    if let Some(result) = result {
        assert!(
            result.inliers.len() >= 4,
            "Should find at least 4 inliers for homography"
        );
    }
}

/// Test progressive RANSAC gives better results with confidence weights
#[test]
fn test_progressive_ransac_uses_weights() {
    // Create points with varying confidence
    let ref_points: Vec<(f64, f64)> = (0..20).map(|i| (i as f64 * 10.0, i as f64 * 5.0)).collect();

    let transform = Transform::translation(5.0, 3.0);
    let mut target_points: Vec<(f64, f64)> = ref_points
        .iter()
        .map(|&(x, y)| transform.apply(x, y))
        .collect();

    // Add noise to low-confidence points (first 5)
    for point in target_points.iter_mut().take(5) {
        point.0 += 50.0;
        point.1 += 50.0;
    }

    // High confidence for good points, low for outliers
    let confidences: Vec<f64> = (0..20).map(|i| if i < 5 { 0.1 } else { 0.9 }).collect();

    let config = RansacConfig {
        max_iterations: 200,
        inlier_threshold: 2.0,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator.estimate_progressive(
        &ref_points,
        &target_points,
        &confidences,
        TransformType::Translation,
    );

    assert!(result.is_some(), "Progressive RANSAC should succeed");
    let result = result.unwrap();

    // Should find the 15 good points as inliers
    assert!(
        result.inliers.len() >= 12,
        "Should find at least 12 of 15 good points as inliers, got {}",
        result.inliers.len()
    );
}

/// Test RANSAC with minimum possible point count
#[test]
fn test_ransac_minimum_points() {
    // Exactly 2 points for translation (minimum required)
    let ref_points = vec![(0.0, 0.0), (100.0, 0.0)];
    let target_points = vec![(10.0, 10.0), (110.0, 10.0)];

    let config = RansacConfig {
        max_iterations: 100,
        inlier_threshold: 1.0,
        min_inlier_ratio: 0.5, // Allow 50% inliers (1 of 2)
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator.estimate(&ref_points, &target_points, TransformType::Translation);

    assert!(result.is_some(), "Should work with minimum 2 points");
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 2);
}

// ============================================================================
// StarMatch-based estimation tests
// ============================================================================

#[test]
fn test_estimate_with_matches_basic() {
    use crate::registration::triangle::StarMatch;

    // Create reference and target stars
    let ref_stars: Vec<(f64, f64)> = vec![
        (100.0, 100.0),
        (200.0, 100.0),
        (100.0, 200.0),
        (200.0, 200.0),
        (150.0, 150.0),
    ];

    // Apply known translation
    let dx = 50.0;
    let dy = -30.0;
    let target_stars: Vec<(f64, f64)> = ref_stars.iter().map(|(x, y)| (x + dx, y + dy)).collect();

    // Create matches with varying confidences
    let matches: Vec<StarMatch> = (0..ref_stars.len())
        .map(|i| StarMatch {
            ref_idx: i,
            target_idx: i,
            votes: 10 - i,                      // Higher votes for lower indices
            confidence: 1.0 - (i as f64 * 0.1), // Higher confidence for lower indices
        })
        .collect();

    let config = RansacConfig {
        seed: Some(42),
        ..Default::default()
    };

    let ransac = RansacEstimator::new(config);
    let result = ransac.estimate_with_matches(
        &matches,
        &ref_stars,
        &target_stars,
        TransformType::Translation,
    );

    assert!(result.is_some(), "estimate_with_matches should succeed");
    let result = result.unwrap();

    // Should find the correct translation
    let (est_dx, est_dy) = result.transform.translation_components();
    assert!(
        (est_dx - dx).abs() < 1.0,
        "Expected dx={}, got {}",
        dx,
        est_dx
    );
    assert!(
        (est_dy - dy).abs() < 1.0,
        "Expected dy={}, got {}",
        dy,
        est_dy
    );
}

#[test]
fn test_estimate_with_matches_empty() {
    use crate::registration::triangle::StarMatch;

    let matches: Vec<StarMatch> = vec![];
    let ref_stars: Vec<(f64, f64)> = vec![];
    let target_stars: Vec<(f64, f64)> = vec![];

    let ransac = RansacEstimator::new(RansacConfig::default());
    let result = ransac.estimate_with_matches(
        &matches,
        &ref_stars,
        &target_stars,
        TransformType::Translation,
    );

    assert!(result.is_none(), "Empty matches should return None");
}

#[test]
fn test_estimate_with_matches_uses_confidence() {
    use crate::registration::triangle::StarMatch;

    // Create points where one outlier has low confidence
    let ref_stars: Vec<(f64, f64)> = vec![
        (100.0, 100.0),
        (200.0, 100.0),
        (100.0, 200.0),
        (200.0, 200.0),
        (150.0, 150.0), // This one will be an outlier
    ];

    let dx = 50.0;
    let dy = -30.0;

    // Create target stars with one outlier
    let mut target_stars: Vec<(f64, f64)> =
        ref_stars.iter().map(|(x, y)| (x + dx, y + dy)).collect();
    target_stars[4] = (1000.0, 1000.0); // Outlier

    // Create matches - give the outlier very low confidence
    let matches: Vec<StarMatch> = (0..ref_stars.len())
        .map(|i| StarMatch {
            ref_idx: i,
            target_idx: i,
            votes: if i == 4 { 1 } else { 10 },
            confidence: if i == 4 { 0.01 } else { 0.9 }, // Very low confidence for outlier
        })
        .collect();

    let config = RansacConfig {
        seed: Some(42),
        inlier_threshold: 5.0,
        ..Default::default()
    };

    let ransac = RansacEstimator::new(config);
    let result = ransac.estimate_with_matches(
        &matches,
        &ref_stars,
        &target_stars,
        TransformType::Translation,
    );

    assert!(result.is_some());
    let result = result.unwrap();

    // The outlier should not be in the inliers
    assert!(
        !result.inliers.contains(&4),
        "Outlier (index 4) should not be in inliers"
    );

    // Should still find the correct translation
    let (est_dx, est_dy) = result.transform.translation_components();
    assert!(
        (est_dx - dx).abs() < 1.0,
        "Expected dx={}, got {}",
        dx,
        est_dx
    );
    assert!(
        (est_dy - dy).abs() < 1.0,
        "Expected dy={}, got {}",
        dy,
        est_dy
    );
}
