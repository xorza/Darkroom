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

    let known = TransformMatrix::similarity(dx, dy, angle, scale);
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
    let known = TransformMatrix::affine([1.2, 0.3, 5.0, -0.1, 0.9, -3.0]);
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
    let known = TransformMatrix::homography([1.1, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
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

    let known = TransformMatrix::similarity(5.0, -3.0, PI / 4.0, 1.2);
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

    let known = TransformMatrix::translation(5.0, 3.0);
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
fn test_compute_residuals() {
    let ref_points = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
    let target_points = vec![(5.0, 0.0), (15.0, 0.0), (5.0, 10.0)];

    let transform = TransformMatrix::translation(5.0, 0.0);
    let residuals = compute_residuals(&ref_points, &target_points, &transform);

    assert_eq!(residuals.len(), 3);
    for r in &residuals {
        assert!(approx_eq(*r, 0.0, EPSILON));
    }
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

    let known = TransformMatrix::affine([1.1, 0.2, 10.0, -0.1, 0.95, 5.0]);
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
    let known = TransformMatrix::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
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
