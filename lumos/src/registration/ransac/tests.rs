//! Tests for RANSAC module.

use super::*;
use crate::registration::triangle::PointMatch;
use glam::DVec2;
use std::f64::consts::PI;

const EPSILON: f64 = 1e-6;

fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() < eps
}

/// Create PointMatch objects from paired point arrays with uniform confidence.
fn make_matches(n: usize) -> Vec<PointMatch> {
    (0..n)
        .map(|i| PointMatch {
            ref_idx: i,
            target_idx: i,
            votes: 1,
            confidence: 1.0,
        })
        .collect()
}

/// Create PointMatch objects with custom confidences.
fn make_matches_with_confidence(confidences: &[f64]) -> Vec<PointMatch> {
    confidences
        .iter()
        .enumerate()
        .map(|(i, &c)| PointMatch {
            ref_idx: i,
            target_idx: i,
            votes: 1,
            confidence: c,
        })
        .collect()
}

/// Helper to call estimate with uniform confidences from raw point arrays.
fn estimate_uniform(
    estimator: &RansacEstimator,
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform_type: TransformType,
) -> Option<RansacResult> {
    let matches = make_matches(ref_points.len());
    estimator.estimate(&matches, ref_points, target_points, transform_type)
}

#[test]
fn test_estimate_translation() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];
    let target_points: Vec<DVec2> = ref_points
        .iter()
        .map(|p| *p + DVec2::new(5.0, -3.0))
        .collect();

    let t = estimate_transform(&ref_points, &target_points, TransformType::Translation).unwrap();
    let d = t.translation_components();

    assert!(approx_eq(d.x, 5.0, EPSILON));
    assert!(approx_eq(d.y, -3.0, EPSILON));
}

#[test]
fn test_estimate_similarity() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    // Apply known similarity transform
    let angle = PI / 6.0; // 30 degrees
    let scale = 1.5;
    let t = DVec2::new(20.0, -10.0);

    let known = Transform::similarity(t, angle, scale);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Similarity).unwrap();

    // Check parameters
    assert!(approx_eq(estimated.rotation_angle(), angle, 0.01));
    assert!(approx_eq(estimated.scale_factor(), scale, 0.01));

    let est_t = estimated.translation_components();
    assert!(approx_eq(est_t.x, t.x, 0.1));
    assert!(approx_eq(est_t.y, t.y, 0.1));
}

#[test]
fn test_estimate_euclidean() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let angle = PI / 12.0; // 15 degrees
    let t = DVec2::new(5.0, -3.0);

    let known = Transform::euclidean(t, angle);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Euclidean).unwrap();

    assert!(approx_eq(estimated.rotation_angle(), angle, 0.01));
    assert!(approx_eq(estimated.scale_factor(), 1.0, 0.01));

    let est_t = estimated.translation_components();
    assert!(approx_eq(est_t.x, t.x, 0.1));
    assert!(approx_eq(est_t.y, t.y, 0.1));
}

/// Verify Euclidean estimation doesn't absorb scale.
/// When data has inherent scale != 1, the Euclidean estimator should still
/// produce scale=1 and recover the best rotation/translation without scale.
#[test]
fn test_estimate_euclidean_ignores_scale() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    // Create target with similarity transform (rotation + scale 1.1)
    let angle = PI / 6.0; // 30 degrees
    let scale = 1.1;
    let t = DVec2::new(3.0, -2.0);
    let sim = Transform::similarity(t, angle, scale);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| sim.apply(p)).collect();

    // Euclidean estimation should NOT absorb the scale
    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Euclidean).unwrap();

    // Scale must be exactly 1.0 (Euclidean constraint)
    assert!(
        approx_eq(estimated.scale_factor(), 1.0, 1e-10),
        "Euclidean scale must be 1.0, got {}",
        estimated.scale_factor()
    );

    // Rotation should still be close to the true angle
    assert!(
        approx_eq(estimated.rotation_angle(), angle, 0.05),
        "Rotation should be close to {}, got {}",
        angle,
        estimated.rotation_angle()
    );
}

#[test]
fn test_estimate_affine() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Apply known affine transform (with shear)
    let known = Transform::affine([1.2, 0.3, 5.0, -0.1, 0.9, -3.0]);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let estimated = estimate_transform(&ref_points, &target_points, TransformType::Affine).unwrap();

    // Check that transform produces correct results
    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(approx_eq(pp.x, tp.x, 0.01));
        assert!(approx_eq(pp.y, tp.y, 0.01));
    }
}

#[test]
fn test_estimate_homography() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(25.0, 75.0),
    ];

    // Apply known homography
    let known = Transform::homography([1.1, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Homography).unwrap();

    // Check that transform produces correct results
    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            approx_eq(pp.x, tp.x, 0.5) && approx_eq(pp.y, tp.y, 0.5),
            "Expected ({}, {}), got ({}, {})",
            tp.x,
            tp.y,
            pp.x,
            pp.y
        );
    }
}

#[test]
fn test_ransac_perfect_translation() {
    let ref_points = vec![
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

    let config = RansacParams {
        seed: Some(42),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    );

    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 8);

    let t = result.transform.translation_components();
    assert!(approx_eq(t.x, 15.0, 0.1));
    assert!(approx_eq(t.y, -7.0, 0.1));
}

#[test]
fn test_ransac_perfect_similarity() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(7.0, 3.0),
        DVec2::new(2.0, 8.0),
        DVec2::new(9.0, 1.0),
    ];

    let known = Transform::similarity(DVec2::new(5.0, -3.0), PI / 4.0, 1.2);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 8);
}

#[test]
fn test_ransac_with_outliers() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(7.0, 3.0),
        DVec2::new(2.0, 8.0),
        DVec2::new(9.0, 1.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 200.0), // Outliers
    ];

    let known = Transform::translation(DVec2::new(5.0, 3.0));
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    // Make last two points outliers
    target_points[8] = DVec2::new(500.0, 500.0);
    target_points[9] = DVec2::new(600.0, 600.0);

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.33,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    );

    assert!(result.is_some());
    let result = result.unwrap();

    // Should have 8 inliers (excluding 2 outliers)
    assert_eq!(result.inliers.len(), 8);
    assert!(!result.inliers.contains(&8));
    assert!(!result.inliers.contains(&9));
}

#[test]
fn test_ransac_insufficient_points() {
    let ref_points = vec![DVec2::new(0.0, 0.0)];
    let target_points = vec![DVec2::new(1.0, 1.0)];

    let estimator = RansacEstimator::new(RansacParams::default());
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(result.is_none());
}

#[test]
fn test_adaptive_iterations() {
    // Formula: N = ceil(log(1-conf) / log(1 - w^n))
    // Edge cases: returns 1
    assert_eq!(adaptive_iterations(0.0, 2, 0.99), 1);
    assert_eq!(adaptive_iterations(1.0, 2, 0.99), 1);

    // w=0.9, n=2, conf=0.99: w^n=0.81
    // N = ceil(ln(0.01)/ln(0.19)) = ceil(4.6052/1.6607) = ceil(2.773) = 3
    assert_eq!(adaptive_iterations(0.9, 2, 0.99), 3);

    // w=0.3, n=2, conf=0.99: w^n=0.09
    // N = ceil(ln(0.01)/ln(0.91)) = ceil(4.6052/0.09431) = ceil(48.83) = 49
    assert_eq!(adaptive_iterations(0.3, 2, 0.99), 49);

    // More iterations with lower inlier ratio
    assert!(adaptive_iterations(0.3, 2, 0.99) > adaptive_iterations(0.9, 2, 0.99));
}

#[test]
fn test_centroid() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(0.0, 10.0),
    ];
    let c = centroid(&points);
    assert!(approx_eq(c.x, 5.0, EPSILON));
    assert!(approx_eq(c.y, 5.0, EPSILON));
}

#[test]
fn test_normalize_points() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(0.0, 10.0),
    ];
    let (normalized, _) = normalize_points(&points);

    // Check centroid is at origin
    let c = centroid(&normalized);
    assert!(approx_eq(c.x, 0.0, EPSILON));
    assert!(approx_eq(c.y, 0.0, EPSILON));

    // Check average distance is sqrt(2)
    let avg_dist: f64 =
        normalized.iter().map(|p| p.length()).sum::<f64>() / normalized.len() as f64;
    assert!(approx_eq(avg_dist, std::f64::consts::SQRT_2, 0.01));
}

#[test]
fn test_ransac_affine() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(25.0, 75.0),
        DVec2::new(75.0, 25.0),
        DVec2::new(33.0, 66.0),
    ];

    let known = Transform::affine([1.1, 0.2, 10.0, -0.1, 0.95, 5.0]);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Affine,
    );

    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 8);
}

#[test]
fn test_ransac_homography() {
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(25.0, 75.0),
        DVec2::new(75.0, 25.0),
        DVec2::new(33.0, 66.0),
    ];

    // Use a mild homography
    let known = Transform::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.33,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Homography,
    );

    assert!(result.is_some());
}

// ============================================================================
// LO-RANSAC Tests
// ============================================================================

#[test]
fn test_lo_ransac_improves_inlier_count() {
    // Create points with some noise
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 20.0;
            let y = (i / 5) as f64 * 20.0;
            DVec2::new(x, y)
        })
        .collect();

    let known = Transform::similarity(DVec2::new(10.0, -5.0), PI / 8.0, 1.1);
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    // Add some noise to make it harder
    target_points[5].x += 0.5;
    target_points[10].y -= 0.3;

    // Test with LO-RANSAC enabled
    let config_with_lo = RansacParams {
        seed: Some(123),
        use_local_optimization: true,
        lo_max_iterations: 5,
        max_sigma: 0.33,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };
    let estimator_with = RansacEstimator::new(config_with_lo);
    let result_with = estimator_with
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Similarity,
        )
        .unwrap();

    // Test with LO-RANSAC disabled
    let config_without_lo = RansacParams {
        seed: Some(123),
        use_local_optimization: false,
        max_sigma: 0.33,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };
    let estimator_without = RansacEstimator::new(config_without_lo);
    let result_without = estimator_without
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Similarity,
        )
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
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(7.0, 3.0),
        DVec2::new(3.0, 7.0),
        DVec2::new(8.0, 8.0),
    ];

    let known = Transform::translation(DVec2::new(5.0, 3.0));
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        use_local_optimization: true,
        lo_max_iterations: 10,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    // All points should be inliers
    assert_eq!(result.inliers.len(), 8);

    // Transform should be accurate
    let t = result.transform.translation_components();
    assert!(approx_eq(t.x, 5.0, 0.1));
    assert!(approx_eq(t.y, 3.0, 0.1));
}

#[test]
fn test_ransac_30_percent_outliers() {
    // 10 inliers, ~4 outliers (30%)
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(20.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(20.0, 10.0),
        DVec2::new(0.0, 20.0),
        DVec2::new(10.0, 20.0),
        DVec2::new(20.0, 20.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(100.0, 100.0), // outlier
        DVec2::new(150.0, 50.0),  // outlier
        DVec2::new(200.0, 200.0), // outlier
        DVec2::new(250.0, 150.0), // outlier
    ];

    let known = Transform::translation(DVec2::new(5.0, 3.0));
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    // Make outliers actually outliers
    target_points[10] = DVec2::new(500.0, 500.0);
    target_points[11] = DVec2::new(600.0, 300.0);
    target_points[12] = DVec2::new(700.0, 700.0);
    target_points[13] = DVec2::new(800.0, 400.0);

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.33,
        use_local_optimization: true,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
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
    let ref_points: Vec<DVec2> = (0..10)
        .map(|i| {
            let x = 2000.0 + (i % 5) as f64 * 100.0;
            let y = 1500.0 + (i / 5) as f64 * 100.0;
            DVec2::new(x, y)
        })
        .collect();

    let known = Transform::similarity(DVec2::new(50.0, -30.0), PI / 16.0, 1.05);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Similarity,
        )
        .unwrap();

    // Should find all points as inliers
    assert_eq!(result.inliers.len(), 10);

    // Check accuracy
    for i in 0..ref_points.len() {
        let r = ref_points[i];
        let t = target_points[i];
        let p = result.transform.apply(r);
        let error = (p - t).length();
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
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];
    let target_points: Vec<DVec2> = ref_points
        .iter()
        .map(|p| DVec2::new(p.x + 5.0, p.y + 3.0))
        .collect();

    let config = RansacParams {
        seed: Some(12345),
        ..Default::default()
    };

    // Run twice with same seed
    let estimator1 = RansacEstimator::new(config.clone());
    let result1 = estimator1
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    let estimator2 = RansacEstimator::new(config);
    let result2 = estimator2
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    // Should get identical results
    assert_eq!(result1.inliers, result2.inliers);
    assert_eq!(result1.iterations, result2.iterations);

    let t1 = result1.transform.translation_components();
    let t2 = result2.transform.translation_components();
    assert!(approx_eq(t1.x, t2.x, EPSILON));
    assert!(approx_eq(t1.y, t2.y, EPSILON));
}

#[test]
fn test_progressive_ransac_basic() {
    // Create a simple translation scenario with confidence scores
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 20.0, (i / 5) as f64 * 20.0))
        .collect();

    let offset = DVec2::new(15.0, -8.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|p| *p + offset).collect();

    // High confidence for all matches (perfect data)
    let confidences: Vec<f64> = vec![0.9; 20];

    let config = RansacParams {
        seed: Some(42),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator
        .estimate(
            &make_matches_with_confidence(&confidences),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    let est = result.transform.translation_components();
    assert!(approx_eq(est.x, offset.x, 0.1));
    assert!(approx_eq(est.y, offset.y, 0.1));
    assert_eq!(result.inliers.len(), 20);
}

#[test]
fn test_progressive_ransac_with_outliers() {
    // Create scenario where high-confidence matches are inliers
    // and low-confidence matches are outliers
    let mut ref_points: Vec<DVec2> = (0..15)
        .map(|i| DVec2::new((i % 5) as f64 * 20.0, (i / 5) as f64 * 20.0))
        .collect();

    let offset = DVec2::new(10.0, 5.0);
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|p| *p + offset).collect();

    // Add 5 outliers with low confidence
    for i in 0..5 {
        ref_points.push(DVec2::new(100.0 + i as f64 * 10.0, 100.0));
        target_points.push(DVec2::new(200.0 + i as f64 * 5.0, 50.0)); // Wrong correspondence
    }

    // High confidence for inliers, low for outliers
    let mut confidences: Vec<f64> = vec![0.9; 15];
    confidences.extend(vec![0.1; 5]);

    let config = RansacParams {
        seed: Some(123),
        max_iterations: 500,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator
        .estimate(
            &make_matches_with_confidence(&confidences),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    let est = result.transform.translation_components();
    assert!(approx_eq(est.x, offset.x, 0.5));
    assert!(approx_eq(est.y, offset.y, 0.5));

    // Should find mostly inliers (the first 15 points)
    assert!(result.inliers.len() >= 12);
}

#[test]
fn test_progressive_ransac_finds_solution_faster() {
    // Progressive RANSAC should find a good solution in fewer iterations
    // when high-confidence matches are correct
    let ref_points: Vec<DVec2> = (0..50)
        .map(|i| DVec2::new((i % 10) as f64 * 10.0, (i / 10) as f64 * 10.0))
        .collect();

    let angle = PI / 12.0; // 15 degrees
    let scale = 1.2;
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle, scale);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    // Varying confidence (higher for more central points)
    let confidences: Vec<f64> = (0..50)
        .map(|i| {
            let x = (i % 10) as f64;
            let y = (i / 10) as f64;
            let dist = ((x - 4.5).powi(2) + (y - 2.0).powi(2)).sqrt();
            1.0 / (1.0 + dist * 0.1)
        })
        .collect();

    let config = RansacParams {
        seed: Some(999),
        max_iterations: 200,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator
        .estimate(
            &make_matches_with_confidence(&confidences),
            &ref_points,
            &target_points,
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
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 1e6;
            let y = (i / 5) as f64 * 1e6;
            DVec2::new(x, y)
        })
        .collect();

    let known = Transform::translation(DVec2::new(5000.0, -3000.0));
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 33.0, // Scaled threshold
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    assert_eq!(result.inliers.len(), 20);
    let t = result.transform.translation_components();
    assert!(approx_eq(t.x, 5000.0, 1.0));
    assert!(approx_eq(t.y, -3000.0, 1.0));
}

#[test]
fn test_ransac_small_coordinates() {
    // Test with small but reasonable coordinates
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 10.0;
            let y = (i / 5) as f64 * 10.0;
            DVec2::new(x, y)
        })
        .collect();

    let known = Transform::translation(DVec2::new(0.5, -0.3));
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.033,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    assert_eq!(result.inliers.len(), 20);
    let t = result.transform.translation_components();
    assert!(approx_eq(t.x, 0.5, 0.01));
    assert!(approx_eq(t.y, -0.3, 0.01));
}

#[test]
fn test_ransac_mixed_scale_coordinates() {
    // Test with points at very different scales (some near origin, some far)
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
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Translation,
        )
        .unwrap();

    assert_eq!(result.inliers.len(), 10);
    let t = result.transform.translation_components();
    assert!(approx_eq(t.x, 10.0, 0.1));
    assert!(approx_eq(t.y, -5.0, 0.1));
}

#[test]
fn test_homography_near_affine() {
    // Homography with very small perspective components (nearly affine)
    let ref_points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(25.0, 75.0),
        DVec2::new(75.0, 25.0),
        DVec2::new(33.0, 66.0),
    ];

    // Homography with tiny perspective components
    let known = Transform::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 1e-8, 1e-8]);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Homography,
        )
        .unwrap();

    // Should still find all points as inliers
    assert!(result.inliers.len() >= 7);

    // Check transform accuracy
    for i in result.inliers.iter() {
        let r = ref_points[*i];
        let t = target_points[*i];
        let p = result.transform.apply(r);
        let error = (p - t).length();
        assert!(error < 1.0, "High error {} at point {}", error, i);
    }
}

#[test]
fn test_similarity_very_small_rotation() {
    // Very small rotation angle (< 0.1 degrees)
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 100.0;
            let y = (i / 5) as f64 * 100.0;
            DVec2::new(x, y)
        })
        .collect();

    let tiny_angle = 0.001; // ~0.057 degrees
    let known = Transform::similarity(DVec2::new(5.0, 3.0), tiny_angle, 1.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Similarity,
        )
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
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = 100.0 + (i % 5) as f64 * 100.0;
            let y = 100.0 + (i / 5) as f64 * 100.0;
            DVec2::new(x, y)
        })
        .collect();

    let tiny_scale = 1.0001; // 0.01% scale difference
    let known = Transform::similarity(DVec2::new(2.0, -1.0), 0.0, tiny_scale);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
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

#[test]
fn test_affine_with_shear() {
    // Affine transform with significant shear
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 50.0;
            let y = (i / 5) as f64 * 50.0;
            DVec2::new(x, y)
        })
        .collect();

    // Shear: x' = x + 0.3*y, y' = y + 0.1*x
    let known = Transform::affine([1.0, 0.3, 10.0, 0.1, 1.0, -5.0]);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator
        .estimate(
            &make_matches(ref_points.len()),
            &ref_points,
            &target_points,
            TransformType::Affine,
        )
        .unwrap();

    assert_eq!(result.inliers.len(), 20);

    // Check all points transform correctly
    for i in 0..ref_points.len() {
        let r = ref_points[i];
        let t = target_points[i];
        let p = result.transform.apply(r);
        let error = (p - t).length();
        assert!(error < 0.1, "High error {} at point {}", error, i);
    }
}

#[test]
fn test_normalize_points_extreme_values() {
    // Test normalization with extreme coordinate values
    let points = vec![
        DVec2::new(1e10, 1e10),
        DVec2::new(1e10 + 1.0, 1e10),
        DVec2::new(1e10, 1e10 + 1.0),
        DVec2::new(1e10 + 1.0, 1e10 + 1.0),
    ];

    let (normalized, transform) = normalize_points(&points);

    // Check centroid is at origin
    let c = centroid(&normalized);
    assert!(c.x.abs() < 1e-10, "Centroid x not at origin: {}", c.x);
    assert!(c.y.abs() < 1e-10, "Centroid y not at origin: {}", c.y);

    // Transform should be invertible (denormalization should recover original)
    let inv = transform.inverse();
    for (orig, norm) in points.iter().zip(normalized.iter()) {
        let r = inv.apply(*norm);
        let d = r - *orig;
        assert!(d.x.abs() < 1e-5, "X mismatch: {} vs {}", r.x, orig.x);
        assert!(d.y.abs() < 1e-5, "Y mismatch: {} vs {}", r.y, orig.y);
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
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(25.0, 75.0),
        DVec2::new(75.0, 25.0),
        DVec2::new(33.0, 66.0),
    ];

    let transform = Transform::similarity(DVec2::new(10.0, -5.0), 0.2, 1.1);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| transform.apply(p)).collect();

    let config = RansacParams {
        max_iterations: 100,
        max_sigma: 0.33,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

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
        DVec2::new(10.0, 20.0),
        DVec2::new(30.0, 40.0),
        DVec2::new(50.0, 60.0),
        DVec2::new(70.0, 80.0),
        DVec2::new(90.0, 100.0),
    ];

    // Target points are completely unrelated
    let target_points = vec![
        DVec2::new(500.0, 600.0),
        DVec2::new(700.0, 100.0),
        DVec2::new(200.0, 900.0),
        DVec2::new(800.0, 50.0),
        DVec2::new(150.0, 350.0),
    ];

    let config = RansacParams {
        max_iterations: 100,
        max_sigma: 0.33,
        min_inlier_ratio: 0.8, // Require 80% inliers
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

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

/// Test adaptive iteration count with exact formula values
#[test]
fn test_adaptive_iteration_count() {
    // w=0.5, n=2, conf=0.999: w^n=0.25
    // N = ceil(ln(0.001)/ln(0.75)) = ceil(6.9078/0.2877) = ceil(24.01) = 25
    assert_eq!(adaptive_iterations(0.5, 2, 0.999), 25);

    // w=0.9, n=2, conf=0.999: w^n=0.81
    // N = ceil(ln(0.001)/ln(0.19)) = ceil(6.9078/1.6607) = ceil(4.16) = 5
    assert_eq!(adaptive_iterations(0.9, 2, 0.999), 5);

    // w=0.1, n=2, conf=0.999: w^n=0.01
    // N = ceil(ln(0.001)/ln(0.99)) = ceil(6.9078/0.01005) = ceil(687.3) = 688
    assert_eq!(adaptive_iterations(0.1, 2, 0.999), 688);

    // Larger sample size requires more iterations at same inlier ratio
    // w=0.5, n=4, conf=0.999: w^n=0.0625
    // N = ceil(ln(0.001)/ln(0.9375)) = ceil(6.9078/0.06454) = ceil(107.0) = 108
    assert_eq!(adaptive_iterations(0.5, 4, 0.999), 108);
}

/// Test that RANSAC early terminates when it finds a good model
#[test]
fn test_ransac_early_termination() {
    // All perfect inliers - should terminate early
    let ref_points: Vec<DVec2> = (0..50)
        .map(|i| DVec2::new(i as f64 * 10.0, i as f64 * 5.0))
        .collect();

    let transform = Transform::translation(DVec2::new(7.0, 3.0));
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| transform.apply(p)).collect();

    let config = RansacParams {
        max_iterations: 10000, // Very high, but should terminate early
        max_sigma: 0.33,
        confidence: 0.999,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    );

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
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 1.0),   // Nearly on x-axis
        DVec2::new(200.0, -1.0),  // Nearly on x-axis
        DVec2::new(300.0, 0.5),   // Nearly on x-axis
        DVec2::new(0.0, 100.0),   // This one breaks collinearity
        DVec2::new(100.0, 100.0), // This one breaks collinearity
    ];

    let transform = Transform::translation(DVec2::new(10.0, 10.0));
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| transform.apply(p)).collect();

    let config = RansacParams {
        max_iterations: 500,
        max_sigma: 0.67,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    // This may or may not succeed depending on which points are sampled
    // Key is it doesn't panic
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Homography,
    );

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
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new(i as f64 * 10.0, i as f64 * 5.0))
        .collect();

    let transform = Transform::translation(DVec2::new(5.0, 3.0));
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|&p| transform.apply(p)).collect();

    // Add noise to low-confidence points (first 5)
    for point in target_points.iter_mut().take(5) {
        point.x += 50.0;
        point.y += 50.0;
    }

    // High confidence for good points, low for outliers
    let confidences: Vec<f64> = (0..20).map(|i| if i < 5 { 0.1 } else { 0.9 }).collect();

    let config = RansacParams {
        max_iterations: 200,
        max_sigma: 0.67,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimator.estimate(
        &make_matches_with_confidence(&confidences),
        &ref_points,
        &target_points,
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
    let ref_points = vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0)];
    let target_points = vec![DVec2::new(10.0, 10.0), DVec2::new(110.0, 10.0)];

    let config = RansacParams {
        max_iterations: 100,
        max_sigma: 0.33,
        min_inlier_ratio: 0.5, // Allow 50% inliers (1 of 2)
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);

    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    );

    assert!(result.is_some(), "Should work with minimum 2 points");
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 2);
}

// ============================================================================
// PointMatch-based estimation tests
// ============================================================================

#[test]
fn test_estimate_basic() {
    use crate::registration::triangle::PointMatch;

    // Create reference and target stars
    let ref_stars: Vec<DVec2> = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 100.0),
        DVec2::new(100.0, 200.0),
        DVec2::new(200.0, 200.0),
        DVec2::new(150.0, 150.0),
    ];

    // Apply known translation
    let offset = DVec2::new(50.0, -30.0);
    let target_stars: Vec<DVec2> = ref_stars.iter().map(|p| *p + offset).collect();

    // Create matches with varying confidences
    let matches: Vec<PointMatch> = (0..ref_stars.len())
        .map(|i| PointMatch {
            ref_idx: i,
            target_idx: i,
            votes: 10 - i,                      // Higher votes for lower indices
            confidence: 1.0 - (i as f64 * 0.1), // Higher confidence for lower indices
        })
        .collect();

    let config = RansacParams {
        seed: Some(42),
        ..Default::default()
    };

    let ransac = RansacEstimator::new(config);
    let result = ransac.estimate(
        &matches,
        &ref_stars,
        &target_stars,
        TransformType::Translation,
    );

    assert!(result.is_some(), "estimate should succeed");
    let result = result.unwrap();

    // Should find the correct translation
    let est = result.transform.translation_components();
    assert!(
        (est.x - offset.x).abs() < 1.0,
        "Expected dx={}, got {}",
        offset.x,
        est.x
    );
    assert!(
        (est.y - offset.y).abs() < 1.0,
        "Expected dy={}, got {}",
        offset.y,
        est.y
    );
}

#[test]
fn test_estimate_empty() {
    use crate::registration::triangle::PointMatch;

    let matches: Vec<PointMatch> = vec![];
    let ref_stars: Vec<DVec2> = vec![];
    let target_stars: Vec<DVec2> = vec![];

    let ransac = RansacEstimator::new(RansacParams::default());
    let result = ransac.estimate(
        &matches,
        &ref_stars,
        &target_stars,
        TransformType::Translation,
    );

    assert!(result.is_none(), "Empty matches should return None");
}

#[test]
fn test_estimate_uses_confidence() {
    use crate::registration::triangle::PointMatch;

    // Create points where one outlier has low confidence
    let ref_stars: Vec<DVec2> = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 100.0),
        DVec2::new(100.0, 200.0),
        DVec2::new(200.0, 200.0),
        DVec2::new(150.0, 150.0), // This one will be an outlier
    ];

    let offset = DVec2::new(50.0, -30.0);

    // Create target stars with one outlier
    let mut target_stars: Vec<DVec2> = ref_stars.iter().map(|p| *p + offset).collect();
    target_stars[4] = DVec2::new(1000.0, 1000.0); // Outlier

    // Create matches - give the outlier very low confidence
    let matches: Vec<PointMatch> = (0..ref_stars.len())
        .map(|i| PointMatch {
            ref_idx: i,
            target_idx: i,
            votes: if i == 4 { 1 } else { 10 },
            confidence: if i == 4 { 0.01 } else { 0.9 }, // Very low confidence for outlier
        })
        .collect();

    let config = RansacParams {
        seed: Some(42),
        max_sigma: 1.67,
        ..Default::default()
    };

    let ransac = RansacEstimator::new(config);
    let result = ransac.estimate(
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
    let est = result.transform.translation_components();
    assert!(
        (est.x - offset.x).abs() < 1.0,
        "Expected dx={}, got {}",
        offset.x,
        est.x
    );
    assert!(
        (est.y - offset.y).abs() < 1.0,
        "Expected dy={}, got {}",
        offset.y,
        est.y
    );
}

// ============================================================================
// Plausibility Check Tests
// ============================================================================

#[test]
fn test_plausibility_rejects_large_rotation() {
    // Create a similarity transform with 30° rotation.
    // Default plausibility limits rotation to ~10°, so RANSAC should fail.
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let known = Transform::similarity(DVec2::new(5.0, -3.0), 30.0_f64.to_radians(), 1.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    // Should fail because all hypotheses have ~30° rotation, exceeding the 10° limit
    assert!(
        result.is_none(),
        "Should reject transforms with rotation exceeding max_rotation"
    );
}

#[test]
fn test_plausibility_rejects_large_scale() {
    // Create a similarity transform with 2x scale.
    // Default plausibility limits scale to (0.8, 1.2).
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let known = Transform::similarity(DVec2::new(5.0, -3.0), 0.0, 2.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: None,
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should reject transforms with scale outside scale_range"
    );
}

#[test]
fn test_plausibility_rejects_small_scale() {
    // Scale of 0.5 should be rejected by (0.8, 1.2) range
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let known = Transform::similarity(DVec2::new(0.0, 0.0), 0.0, 0.5);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: None,
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should reject transforms with scale below scale_range minimum"
    );
}

#[test]
fn test_plausibility_accepts_within_bounds() {
    // 5° rotation and 1.1 scale should pass default-like bounds
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let angle = 5.0_f64.to_radians();
    let scale = 1.1;
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle, scale);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(result.is_some(), "Should accept transforms within bounds");
    let result = result.unwrap();
    assert!(approx_eq(result.transform.rotation_angle(), angle, 0.02));
    assert!(approx_eq(result.transform.scale_factor(), scale, 0.02));
}

#[test]
fn test_plausibility_disabled_accepts_everything() {
    // With both checks disabled (None), any transform should be accepted
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let known = Transform::similarity(DVec2::new(5.0, -3.0), PI / 4.0, 2.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_some(),
        "Should accept any transform when plausibility checks are disabled"
    );
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 20);
}

#[test]
fn test_plausibility_rotation_boundary() {
    // Test at exactly the rotation boundary: 9.9° should pass, 10.1° should fail
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let max_rotation = 10.0_f64.to_radians();

    // 9.9° - should pass
    let angle_pass = 9.9_f64.to_radians();
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle_pass, 1.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(max_rotation),
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );
    assert!(result.is_some(), "9.9° rotation should pass 10° limit");

    // 10.5° - should fail
    let angle_fail = 10.5_f64.to_radians();
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle_fail, 1.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(max_rotation),
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );
    assert!(result.is_none(), "10.5° rotation should fail 10° limit");
}

#[test]
fn test_plausibility_negative_rotation() {
    // Negative rotation should also be checked (absolute value)
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let known = Transform::similarity(DVec2::new(5.0, -3.0), -30.0_f64.to_radians(), 1.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should reject negative rotation exceeding max_rotation"
    );
}

#[test]
fn test_plausibility_scale_boundary() {
    // Test at scale boundaries
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    // 1.15 scale - should pass (0.8, 1.2) range
    let known = Transform::similarity(DVec2::new(0.0, 0.0), 0.0, 1.15);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: None,
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );
    assert!(result.is_some(), "1.15 scale should pass (0.8, 1.2) range");

    // 1.25 scale - should fail (0.8, 1.2) range
    let known = Transform::similarity(DVec2::new(0.0, 0.0), 0.0, 1.25);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: None,
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );
    assert!(result.is_none(), "1.25 scale should fail (0.8, 1.2) range");
}

#[test]
fn test_plausibility_translation_unaffected() {
    // Pure translation should always pass plausibility checks
    // (rotation ~ 0, scale ~ 1)
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let target_points: Vec<DVec2> = ref_points
        .iter()
        .map(|p| *p + DVec2::new(100.0, -50.0))
        .collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(1.0_f64.to_radians()), // Very tight rotation limit
        scale_range: Some((0.99, 1.01)),          // Very tight scale range
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Translation,
    );

    assert!(
        result.is_some(),
        "Pure translation should always pass plausibility checks"
    );
    assert_eq!(result.unwrap().inliers.len(), 20);
}

#[test]
fn test_plausibility_progressive_ransac_rejects() {
    // Progressive RANSAC should also respect plausibility checks
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let known = Transform::similarity(DVec2::new(5.0, -3.0), 30.0_f64.to_radians(), 1.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();
    let confidences: Vec<f64> = vec![0.9; 20];

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: None,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator.estimate(
        &make_matches_with_confidence(&confidences),
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Progressive RANSAC should also reject implausible transforms"
    );
}

#[test]
fn test_plausibility_progressive_ransac_accepts() {
    // Progressive RANSAC should accept plausible transforms
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let angle = 5.0_f64.to_radians();
    let scale = 1.05;
    let known = Transform::similarity(DVec2::new(5.0, -3.0), angle, scale);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();
    let confidences: Vec<f64> = vec![0.9; 20];

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimator.estimate(
        &make_matches_with_confidence(&confidences),
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_some(),
        "Progressive RANSAC should accept plausible transforms"
    );
    let result = result.unwrap();
    assert_eq!(result.inliers.len(), 20);
}

#[test]
fn test_plausibility_combined_rotation_and_scale() {
    // Both rotation and scale checks active simultaneously
    let ref_points: Vec<DVec2> = (0..20)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    // Rotation OK (5°) but scale too large (1.5) - should fail
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 5.0_f64.to_radians(), 1.5);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should fail when scale is out of range even if rotation is OK"
    );

    // Scale OK (1.1) but rotation too large (20°) - should fail
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 20.0_f64.to_radians(), 1.1);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_none(),
        "Should fail when rotation is out of range even if scale is OK"
    );

    // Both within range - should pass
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 5.0_f64.to_radians(), 1.1);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );

    assert!(
        result.is_some(),
        "Should pass when both rotation and scale are within bounds"
    );
}

#[test]
fn test_plausibility_with_outliers_filters_bad_hypotheses() {
    // Mix of inlier and outlier correspondences.
    // Without plausibility, RANSAC might occasionally pick outlier pairs
    // that produce wild transforms. With plausibility enabled, those
    // hypotheses get filtered before expensive inlier counting.
    let ref_points: Vec<DVec2> = (0..15)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    // Small translation (within plausibility bounds)
    let offset = DVec2::new(10.0, -5.0);
    let mut target_points: Vec<DVec2> = ref_points.iter().map(|p| *p + offset).collect();

    // Add outliers that would produce implausible transforms if sampled
    let mut ref_with_outliers = ref_points.clone();
    ref_with_outliers.push(DVec2::new(100.0, 100.0));
    ref_with_outliers.push(DVec2::new(200.0, 50.0));
    target_points.push(DVec2::new(500.0, -300.0)); // Would create huge rotation if paired
    target_points.push(DVec2::new(-100.0, 800.0));

    let config = RansacParams {
        seed: Some(42),
        max_rotation: Some(5.0_f64.to_radians()),
        scale_range: Some((0.9, 1.1)),
        max_sigma: 0.67,
        ..Default::default()
    };
    let estimator = RansacEstimator::new(config);
    let result = estimate_uniform(
        &estimator,
        &ref_with_outliers,
        &target_points,
        TransformType::Translation,
    );

    assert!(
        result.is_some(),
        "Should find the correct translation despite outliers"
    );
    let result = result.unwrap();
    assert!(
        result.inliers.len() >= 13,
        "Should find most of the 15 inliers, got {}",
        result.inliers.len()
    );
}

// ── Degeneracy check tests ──────────────────────────────────────────

#[test]
fn test_degenerate_single_point() {
    // Single point is never degenerate
    assert!(!is_sample_degenerate(&[DVec2::new(1.0, 2.0)]));
}

#[test]
fn test_degenerate_coincident_pair() {
    // Two nearly-identical points are degenerate
    let pts = vec![DVec2::new(5.0, 5.0), DVec2::new(5.0, 5.5)];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_non_degenerate_pair() {
    // Two points far apart are fine
    let pts = vec![DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0)];
    assert!(!is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_collinear_triple() {
    // Three collinear points are degenerate even if well-spaced
    let pts = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(20.0, 0.0),
    ];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_non_degenerate_triangle() {
    // Three points forming a triangle are fine
    let pts = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(5.0, 10.0),
    ];
    assert!(!is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_coincident_in_quad() {
    // Four points where two are nearly coincident
    let pts = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(0.1, 0.1), // too close to first
    ];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_collinear_quad() {
    // Four collinear points
    let pts = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(5.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(15.0, 0.0),
    ];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_non_degenerate_quad() {
    let pts = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(0.0, 100.0),
    ];
    assert!(!is_sample_degenerate(&pts));
}

#[test]
fn test_random_sample_into_produces_unique_indices() {
    use rand::SeedableRng;
    let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
    let n = 50;
    let k = 4;
    let mut buffer = Vec::new();
    let mut indices = Vec::new();

    for _ in 0..200 {
        random_sample_into(&mut rng, n, k, &mut buffer, &mut indices);

        assert_eq!(buffer.len(), k);
        // All indices in range
        for &idx in &buffer {
            assert!(idx < n, "Index {idx} out of range 0..{n}");
        }
        // All indices unique
        let mut sorted = buffer.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), k, "Duplicate indices in sample: {:?}", buffer);
        // Persistent array stays valid
        assert_eq!(indices.len(), n);
        for (i, &v) in indices.iter().enumerate() {
            assert_eq!(
                v, i,
                "indices[{i}] = {v}, expected {i} (corrupted after sample)"
            );
        }
    }
}

/// Test homography estimation with ill-conditioned points (high dynamic range).
/// This is where direct SVD of A outperforms SVD of A^T A (condition number κ vs κ²).
#[test]
fn test_homography_ill_conditioned() {
    // Points spanning a large range — stresses numerical stability
    let ref_points = vec![
        DVec2::new(0.01, 0.02),
        DVec2::new(5000.0, 0.01),
        DVec2::new(5000.0, 4000.0),
        DVec2::new(0.01, 4000.0),
        DVec2::new(2500.0, 2000.0),
        DVec2::new(1000.0, 3000.0),
        DVec2::new(4000.0, 1000.0),
        DVec2::new(100.0, 100.0),
    ];

    // Apply a known homography with perspective
    let known = Transform::homography([1.05, 0.02, 10.0, -0.01, 0.98, 5.0, 1e-5, -2e-5]);
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| known.apply(p)).collect();

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Homography).unwrap();

    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            approx_eq(pp.x, tp.x, 0.5) && approx_eq(pp.y, tp.y, 0.5),
            "Point ({}, {}): expected ({:.4}, {:.4}), got ({:.4}, {:.4})",
            rp.x,
            rp.y,
            tp.x,
            tp.y,
            pp.x,
            pp.y,
        );
    }
}
