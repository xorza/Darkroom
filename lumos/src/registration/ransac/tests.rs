//! Tests for RANSAC module.

use super::*;
use crate::registration::triangle::PointMatch;
use glam::DVec2;
use std::f64::consts::PI;

const TOL: f64 = 1e-6;

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

/// Generate a grid of points for testing.
fn make_grid(cols: usize, rows: usize, spacing: f64) -> Vec<DVec2> {
    let mut points = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            points.push(DVec2::new(c as f64 * spacing, r as f64 * spacing));
        }
    }
    points
}

/// Apply a transform to all points.
fn apply_all(transform: &Transform, points: &[DVec2]) -> Vec<DVec2> {
    points.iter().map(|&p| transform.apply(p)).collect()
}

// ============================================================================
// centroid tests
// ============================================================================

#[test]
fn test_centroid_empty() {
    assert_eq!(centroid(&[]), DVec2::ZERO);
}

#[test]
fn test_centroid_single_point() {
    let c = centroid(&[DVec2::new(7.0, -3.0)]);
    // centroid of a single point is the point itself
    assert!((c.x - 7.0).abs() < TOL);
    assert!((c.y - (-3.0)).abs() < TOL);
}

#[test]
fn test_centroid_square() {
    // centroid of (0,0), (10,0), (10,10), (0,10) = (5, 5)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(0.0, 10.0),
    ];
    let c = centroid(&points);
    // (0+10+10+0)/4 = 5, (0+0+10+10)/4 = 5
    assert!((c.x - 5.0).abs() < TOL);
    assert!((c.y - 5.0).abs() < TOL);
}

#[test]
fn test_centroid_asymmetric() {
    // Three points: (1,2), (3,4), (5,6)
    // centroid = ((1+3+5)/3, (2+4+6)/3) = (3.0, 4.0)
    let points = [
        DVec2::new(1.0, 2.0),
        DVec2::new(3.0, 4.0),
        DVec2::new(5.0, 6.0),
    ];
    let c = centroid(&points);
    assert!((c.x - 3.0).abs() < TOL);
    assert!((c.y - 4.0).abs() < TOL);
}

// ============================================================================
// normalize_points tests
// ============================================================================

#[test]
fn test_normalize_points_empty() {
    let (normalized, transform) = normalize_points(&[]);
    assert!(normalized.is_empty());
    // Transform should be identity
    let p = transform.apply(DVec2::new(1.0, 2.0));
    assert!((p.x - 1.0).abs() < TOL);
    assert!((p.y - 2.0).abs() < TOL);
}

#[test]
fn test_normalize_points_coincident() {
    // All points at same location: avg_dist=0 → returns identity transform
    let points = vec![DVec2::new(5.0, 5.0); 4];
    let (normalized, _) = normalize_points(&points);
    // Should return original points unchanged
    for p in &normalized {
        assert!((p.x - 5.0).abs() < TOL);
        assert!((p.y - 5.0).abs() < TOL);
    }
}

#[test]
fn test_normalize_points_centroid_and_avg_distance() {
    // Points: (0,0), (10,0), (10,10), (0,10)
    // Centroid: (5, 5)
    // Centered: (-5,-5), (5,-5), (5,5), (-5,5)
    // Distances from origin: all sqrt(50) = 5*sqrt(2)
    // avg_dist = 5*sqrt(2)
    // scale = sqrt(2) / (5*sqrt(2)) = 1/5 = 0.2
    // Normalized: (-1,-1), (1,-1), (1,1), (-1,1)
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(0.0, 10.0),
    ];
    let (normalized, transform) = normalize_points(&points);

    // Centroid of normalized points should be at origin
    let c = centroid(&normalized);
    assert!(c.x.abs() < TOL);
    assert!(c.y.abs() < TOL);

    // Average distance should be sqrt(2)
    let avg_dist: f64 =
        normalized.iter().map(|p| p.length()).sum::<f64>() / normalized.len() as f64;
    assert!((avg_dist - std::f64::consts::SQRT_2).abs() < 0.001);

    // Check exact normalized coordinates
    assert!((normalized[0].x - (-1.0)).abs() < TOL); // (0-5)*0.2 = -1
    assert!((normalized[0].y - (-1.0)).abs() < TOL);
    assert!((normalized[1].x - 1.0).abs() < TOL); // (10-5)*0.2 = 1
    assert!((normalized[1].y - (-1.0)).abs() < TOL);

    // Inverse transform should recover original points
    let inv = transform.inverse();
    for (orig, norm) in points.iter().zip(normalized.iter()) {
        let r = inv.apply(*norm);
        assert!((r.x - orig.x).abs() < 1e-8);
        assert!((r.y - orig.y).abs() < 1e-8);
    }
}

#[test]
fn test_normalize_points_extreme_values() {
    // Points with large coordinates
    let points = vec![
        DVec2::new(1e10, 1e10),
        DVec2::new(1e10 + 1.0, 1e10),
        DVec2::new(1e10, 1e10 + 1.0),
        DVec2::new(1e10 + 1.0, 1e10 + 1.0),
    ];

    let (normalized, transform) = normalize_points(&points);

    // Centroid at origin
    let c = centroid(&normalized);
    assert!(c.x.abs() < 1e-10, "Centroid x not at origin: {}", c.x);
    assert!(c.y.abs() < 1e-10, "Centroid y not at origin: {}", c.y);

    // Inverse recovers original (may lose some precision at 1e10 scale)
    let inv = transform.inverse();
    for (orig, norm) in points.iter().zip(normalized.iter()) {
        let r = inv.apply(*norm);
        assert!((r.x - orig.x).abs() < 1e-3, "X: {} vs {}", r.x, orig.x);
        assert!((r.y - orig.y).abs() < 1e-3, "Y: {} vs {}", r.y, orig.y);
    }
}

// ============================================================================
// is_sample_degenerate tests
// ============================================================================

#[test]
fn test_degenerate_empty() {
    assert!(!is_sample_degenerate(&[]));
}

#[test]
fn test_degenerate_single_point() {
    assert!(!is_sample_degenerate(&[DVec2::new(1.0, 2.0)]));
}

#[test]
fn test_degenerate_coincident_pair() {
    // Distance = sqrt((0.5)^2 + (0)^2) = 0.5 < 1.0 (MIN_DIST_SQ threshold)
    let pts = [DVec2::new(5.0, 5.0), DVec2::new(5.0, 5.5)];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_pair_exactly_at_threshold() {
    // Distance = 1.0, dist_sq = 1.0 → NOT less than MIN_DIST_SQ (1.0), so non-degenerate
    let pts = [DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    assert!(!is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_pair_just_below_threshold() {
    // Distance = 0.99, dist_sq = 0.9801 < 1.0 → degenerate
    let pts = [DVec2::new(0.0, 0.0), DVec2::new(0.99, 0.0)];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_non_degenerate_pair() {
    let pts = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0)];
    assert!(!is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_collinear_triple() {
    // Three points on x-axis: cross product = 0 for all
    let pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(20.0, 0.0),
    ];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_collinear_diagonal() {
    // Three points on y=x line
    // v0 = (10,10), v = (20,20), cross = 10*20 - 10*20 = 0
    let pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(20.0, 20.0),
    ];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_non_degenerate_triangle() {
    // (0,0), (10,0), (5,10) → v0=(10,0), v=(5,10), cross = 10*10 - 0*5 = 100 > 1
    let pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(5.0, 10.0),
    ];
    assert!(!is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_coincident_in_quad() {
    // Fourth point too close to first: dist = sqrt(0.1^2+0.1^2) = 0.1414 < 1
    let pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(0.1, 0.1),
    ];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_degenerate_collinear_quad() {
    let pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(5.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(15.0, 0.0),
    ];
    assert!(is_sample_degenerate(&pts));
}

#[test]
fn test_non_degenerate_quad() {
    let pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(0.0, 100.0),
    ];
    assert!(!is_sample_degenerate(&pts));
}

// ============================================================================
// adaptive_iterations tests
// ============================================================================

#[test]
fn test_adaptive_iterations_edge_cases() {
    // w=0 or w=1 → returns 1
    assert_eq!(adaptive_iterations(0.0, 2, 0.99), 1);
    assert_eq!(adaptive_iterations(1.0, 2, 0.99), 1);
}

#[test]
fn test_adaptive_iterations_hand_computed() {
    // Formula: N = ceil(log(1-conf) / log(1 - w^n))

    // w=0.9, n=2, conf=0.99: w^n = 0.81
    // N = ceil(ln(0.01) / ln(0.19)) = ceil(-4.6052 / -1.6607) = ceil(2.773) = 3
    assert_eq!(adaptive_iterations(0.9, 2, 0.99), 3);

    // w=0.3, n=2, conf=0.99: w^n = 0.09
    // N = ceil(ln(0.01) / ln(0.91)) = ceil(-4.6052 / -0.09431) = ceil(48.83) = 49
    assert_eq!(adaptive_iterations(0.3, 2, 0.99), 49);

    // w=0.5, n=2, conf=0.999: w^n = 0.25
    // N = ceil(ln(0.001) / ln(0.75)) = ceil(-6.9078 / -0.28768) = ceil(24.01) = 25
    assert_eq!(adaptive_iterations(0.5, 2, 0.999), 25);

    // w=0.9, n=2, conf=0.999: w^n = 0.81
    // N = ceil(ln(0.001) / ln(0.19)) = ceil(-6.9078 / -1.6607) = ceil(4.16) = 5
    assert_eq!(adaptive_iterations(0.9, 2, 0.999), 5);

    // w=0.1, n=2, conf=0.999: w^n = 0.01
    // N = ceil(ln(0.001) / ln(0.99)) = ceil(-6.9078 / -0.01005) = ceil(687.3) = 688
    assert_eq!(adaptive_iterations(0.1, 2, 0.999), 688);
}

#[test]
fn test_adaptive_iterations_larger_sample_size() {
    // Larger sample size → more iterations at same inlier ratio

    // w=0.5, n=4, conf=0.999: w^n = 0.0625
    // N = ceil(ln(0.001) / ln(0.9375)) = ceil(-6.9078 / -0.06454) = ceil(107.03) = 108
    assert_eq!(adaptive_iterations(0.5, 4, 0.999), 108);

    // w=0.5, n=3, conf=0.999: w^n = 0.125
    // N = ceil(ln(0.001) / ln(0.875)) = ceil(-6.9078 / -0.13353) = ceil(51.73) = 52
    assert_eq!(adaptive_iterations(0.5, 3, 0.999), 52);

    // More points needed → more iterations
    assert!(adaptive_iterations(0.5, 4, 0.999) > adaptive_iterations(0.5, 3, 0.999));
    assert!(adaptive_iterations(0.5, 3, 0.999) > adaptive_iterations(0.5, 2, 0.999));
}

#[test]
fn test_adaptive_iterations_monotonic_in_inlier_ratio() {
    // Higher inlier ratio → fewer iterations needed
    let low = adaptive_iterations(0.3, 2, 0.99); // 49
    let mid = adaptive_iterations(0.5, 2, 0.99); // lower
    let high = adaptive_iterations(0.9, 2, 0.99); // 3
    assert!(low > mid);
    assert!(mid > high);
}

// ============================================================================
// estimate_transform tests (direct, no RANSAC)
// ============================================================================

#[test]
fn test_estimate_translation_hand_computed() {
    // Translation is the average displacement.
    // ref: (0,0), (10,0), (0,10), (10,10)
    // target: (5,-3), (15,-3), (5,7), (15,7)  (offset +5, -3)
    // avg displacement = ((5+5+5+5)/4, (-3-3-3-3)/4) = (5, -3)
    let ref_points = [
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

    assert!((d.x - 5.0).abs() < TOL);
    assert!((d.y - (-3.0)).abs() < TOL);
}

#[test]
fn test_estimate_translation_insufficient_points() {
    let result = estimate_transform(&[], &[], TransformType::Translation);
    assert!(result.is_none());
}

#[test]
fn test_estimate_euclidean_hand_computed() {
    // Euclidean: rotation + translation, scale = 1
    let angle = PI / 12.0; // 15 degrees
    let t = DVec2::new(5.0, -3.0);

    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let known = Transform::euclidean(t, angle);
    let target_points = apply_all(&known, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Euclidean).unwrap();

    assert!(
        (estimated.rotation_angle() - angle).abs() < 1e-10,
        "angle: expected {}, got {}",
        angle,
        estimated.rotation_angle()
    );
    assert!(
        (estimated.scale_factor() - 1.0).abs() < 1e-10,
        "scale: expected 1.0, got {}",
        estimated.scale_factor()
    );

    let est_t = estimated.translation_components();
    assert!((est_t.x - t.x).abs() < 1e-10);
    assert!((est_t.y - t.y).abs() < 1e-10);
}

#[test]
fn test_estimate_euclidean_insufficient_points() {
    let result = estimate_transform(
        &[DVec2::new(0.0, 0.0)],
        &[DVec2::new(1.0, 1.0)],
        TransformType::Euclidean,
    );
    assert!(result.is_none());
}

#[test]
fn test_estimate_euclidean_ignores_scale() {
    // When data has inherent scale != 1, Euclidean estimator should still produce scale=1.
    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let angle = PI / 6.0; // 30 degrees
    let sim = Transform::similarity(DVec2::new(3.0, -2.0), angle, 1.1);
    let target_points = apply_all(&sim, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Euclidean).unwrap();

    // Scale must be exactly 1.0 (Euclidean constraint)
    assert!(
        (estimated.scale_factor() - 1.0).abs() < 1e-10,
        "Euclidean scale must be 1.0, got {}",
        estimated.scale_factor()
    );

    // Rotation should still be close to the true angle
    assert!(
        (estimated.rotation_angle() - angle).abs() < 0.05,
        "Rotation: expected ~{}, got {}",
        angle,
        estimated.rotation_angle()
    );
}

#[test]
fn test_estimate_similarity_hand_computed() {
    let angle = PI / 6.0; // 30 degrees
    let scale = 1.5;
    let t = DVec2::new(20.0, -10.0);

    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let known = Transform::similarity(t, angle, scale);
    let target_points = apply_all(&known, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Similarity).unwrap();

    assert!(
        (estimated.rotation_angle() - angle).abs() < 1e-10,
        "angle: expected {}, got {}",
        angle,
        estimated.rotation_angle()
    );
    assert!(
        (estimated.scale_factor() - scale).abs() < 1e-10,
        "scale: expected {}, got {}",
        scale,
        estimated.scale_factor()
    );

    let est_t = estimated.translation_components();
    assert!((est_t.x - t.x).abs() < 1e-9);
    assert!((est_t.y - t.y).abs() < 1e-9);
}

#[test]
fn test_estimate_similarity_insufficient_points() {
    let result = estimate_transform(
        &[DVec2::new(0.0, 0.0)],
        &[DVec2::new(1.0, 1.0)],
        TransformType::Similarity,
    );
    assert!(result.is_none());
}

#[test]
fn test_estimate_affine_hand_computed() {
    // Affine: [a,b,tx,c,d,ty] → x' = a*x + b*y + tx, y' = c*x + d*y + ty
    // [1.2, 0.3, 5.0, -0.1, 0.9, -3.0]
    let params = [1.2, 0.3, 5.0, -0.1, 0.9, -3.0];
    let known = Transform::affine(params);

    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let target_points = apply_all(&known, &ref_points);

    let estimated = estimate_transform(&ref_points, &target_points, TransformType::Affine).unwrap();

    // Verify each point maps correctly
    // (0,0) → (1.2*0+0.3*0+5, -0.1*0+0.9*0-3) = (5, -3)
    // (10,0) → (12+0+5, -1+0-3) = (17, -4)
    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            (pp.x - tp.x).abs() < 1e-8 && (pp.y - tp.y).abs() < 1e-8,
            "At ({},{}): expected ({},{}), got ({},{})",
            rp.x,
            rp.y,
            tp.x,
            tp.y,
            pp.x,
            pp.y
        );
    }
}

#[test]
fn test_estimate_affine_insufficient_points() {
    let ref_pts = [DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let tar_pts = [DVec2::new(1.0, 1.0), DVec2::new(2.0, 1.0)];
    let result = estimate_transform(&ref_pts, &tar_pts, TransformType::Affine);
    assert!(result.is_none());
}

#[test]
fn test_estimate_homography_hand_computed() {
    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(25.0, 75.0),
    ];

    let known = Transform::homography([1.1, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
    let target_points = apply_all(&known, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Homography).unwrap();

    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            (pp.x - tp.x).abs() < 0.5 && (pp.y - tp.y).abs() < 0.5,
            "At ({},{}): expected ({:.4},{:.4}), got ({:.4},{:.4})",
            rp.x,
            rp.y,
            tp.x,
            tp.y,
            pp.x,
            pp.y
        );
    }
}

#[test]
fn test_estimate_homography_insufficient_points() {
    let ref_pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
    ];
    let tar_pts = [
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 1.0),
        DVec2::new(1.0, 2.0),
    ];
    let result = estimate_transform(&ref_pts, &tar_pts, TransformType::Homography);
    assert!(result.is_none());
}

#[test]
fn test_estimate_homography_ill_conditioned() {
    // Points spanning a large range -- stresses numerical stability
    let ref_points = [
        DVec2::new(0.01, 0.02),
        DVec2::new(5000.0, 0.01),
        DVec2::new(5000.0, 4000.0),
        DVec2::new(0.01, 4000.0),
        DVec2::new(2500.0, 2000.0),
        DVec2::new(1000.0, 3000.0),
        DVec2::new(4000.0, 1000.0),
        DVec2::new(100.0, 100.0),
    ];

    let known = Transform::homography([1.05, 0.02, 10.0, -0.01, 0.98, 5.0, 1e-5, -2e-5]);
    let target_points = apply_all(&known, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Homography).unwrap();

    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            (pp.x - tp.x).abs() < 0.5 && (pp.y - tp.y).abs() < 0.5,
            "At ({},{:.1}): expected ({:.4},{:.4}), got ({:.4},{:.4})",
            rp.x,
            rp.y,
            tp.x,
            tp.y,
            pp.x,
            pp.y,
        );
    }
}

// ============================================================================
// score_hypothesis tests
// ============================================================================

#[test]
fn test_score_hypothesis_perfect_match() {
    // All points map exactly → all residuals = 0 → loss per point = 0
    // score = -total_loss = 0
    let ref_pts = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0)];
    let target_pts = [DVec2::new(5.0, 0.0), DVec2::new(15.0, 0.0)];
    let transform = Transform::translation(DVec2::new(5.0, 0.0));
    let scorer = MagsacScorer::new(1.0);
    let mut inliers = Vec::new();

    let score = score_hypothesis(
        &ref_pts,
        &target_pts,
        &transform,
        &scorer,
        &mut inliers,
        f64::NEG_INFINITY,
    );

    // Perfect match: all residuals = 0, loss = 0, score = -0 = 0
    assert!((score - 0.0).abs() < TOL);
    assert_eq!(inliers.len(), 2);
    assert_eq!(inliers, vec![0, 1]);
}

#[test]
fn test_score_hypothesis_with_one_outlier() {
    // 3 points: first 2 match perfectly, third is an outlier
    let ref_pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(20.0, 0.0),
    ];
    let target_pts = [
        DVec2::new(5.0, 0.0),   // matches with tx=5
        DVec2::new(15.0, 0.0),  // matches with tx=5
        DVec2::new(500.0, 0.0), // outlier: residual = |25 - 500| = 475
    ];
    let transform = Transform::translation(DVec2::new(5.0, 0.0));
    let scorer = MagsacScorer::new(1.0);
    let mut inliers = Vec::new();

    let score = score_hypothesis(
        &ref_pts,
        &target_pts,
        &transform,
        &scorer,
        &mut inliers,
        f64::NEG_INFINITY,
    );

    // First 2 points: loss = 0 each
    // Third point: residual_sq = 475^2 = 225625 >> threshold_sq (9.21)
    //   → outlier_loss = 0.5
    // Total loss = 0 + 0 + 0.5 = 0.5, score = -0.5
    assert!((score - (-0.5)).abs() < TOL);
    assert_eq!(inliers.len(), 2);
    assert_eq!(inliers, vec![0, 1]);
}

#[test]
fn test_score_hypothesis_early_exit() {
    // With a tight best_score, the function should exit early
    let n = 100;
    let ref_pts: Vec<DVec2> = (0..n).map(|i| DVec2::new(i as f64, 0.0)).collect();
    // All points are huge outliers (residual_sq >> threshold)
    let target_pts: Vec<DVec2> = (0..n).map(|i| DVec2::new(i as f64 + 1000.0, 0.0)).collect();
    let transform = Transform::translation(DVec2::new(0.0, 0.0)); // wrong transform
    let scorer = MagsacScorer::new(1.0);
    let mut inliers = Vec::new();

    // best_score = -1.0 means budget = 1.0
    // Each outlier adds 0.5, so after 2 points total_loss = 1.0, exceeding budget
    let score = score_hypothesis(
        &ref_pts,
        &target_pts,
        &transform,
        &scorer,
        &mut inliers,
        -1.0,
    );

    // Should have exited early, score should be <= -1.0
    assert!(score <= -1.0);
    // Inliers buffer should be incomplete (early exit)
    assert!(inliers.len() < n);
}

// ============================================================================
// random_sample_into tests
// ============================================================================

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
        assert_eq!(sorted.len(), k, "Duplicate indices: {:?}", buffer);
        // Persistent array stays valid (undone swaps)
        assert_eq!(indices.len(), n);
        for (i, &v) in indices.iter().enumerate() {
            assert_eq!(v, i, "indices[{i}] = {v}, expected {i}");
        }
    }
}

#[test]
fn test_random_sample_into_k_equals_n() {
    // When k == n, should return all indices (in some order)
    use rand::SeedableRng;
    let mut rng = rand::rngs::SmallRng::seed_from_u64(99);
    let n = 5;
    let k = 5;
    let mut buffer = Vec::new();
    let mut indices = Vec::new();

    random_sample_into(&mut rng, n, k, &mut buffer, &mut indices);

    assert_eq!(buffer.len(), 5);
    let mut sorted = buffer.clone();
    sorted.sort();
    assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
}

// ============================================================================
// weighted_sample_into tests
// ============================================================================

#[test]
fn test_weighted_sample_into_pool_smaller_than_k() {
    // When pool.len() <= k, should return all pool elements
    use rand::SeedableRng;
    let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
    let pool = vec![5, 10, 15];
    let weights = vec![0.0; 20]; // weights indexed by pool values
    let mut buffer = Vec::new();

    weighted_sample_into(&mut rng, &pool, &weights, 5, &mut buffer);
    let mut sorted = buffer.clone();
    sorted.sort();
    assert_eq!(sorted, vec![5, 10, 15]);
}

#[test]
fn test_weighted_sample_into_returns_k_unique() {
    use rand::SeedableRng;
    let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
    let pool: Vec<usize> = (0..20).collect();
    let weights: Vec<f64> = (0..20).map(|i| i as f64 + 1.0).collect();
    let k = 4;
    let mut buffer = Vec::new();

    for _ in 0..100 {
        weighted_sample_into(&mut rng, &pool, &weights, k, &mut buffer);
        assert_eq!(buffer.len(), k);

        // All unique
        let mut sorted = buffer.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            k,
            "Duplicates in weighted sample: {:?}",
            buffer
        );

        // All from pool
        for &idx in &buffer {
            assert!(idx < 20);
        }
    }
}

// ============================================================================
// RANSAC full pipeline tests
// ============================================================================

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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.33,
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.33,
        use_local_optimization: true,
        ..Default::default()
    });
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
fn test_ransac_empty_matches() {
    let matches: Vec<PointMatch> = vec![];
    let ransac = RansacEstimator::new(RansacParams::default());
    let result = ransac.estimate(&matches, &[], &[], TransformType::Translation);
    assert!(result.is_none());
}

#[test]
fn test_ransac_minimum_points_for_translation() {
    // Translation needs 1 point minimum (min_points), but RANSAC samples 1 point.
    // With 2 points, sampling is guaranteed to succeed.
    let ref_points = [DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0)];
    let target_points = [DVec2::new(10.0, 10.0), DVec2::new(110.0, 10.0)];

    let estimator = RansacEstimator::new(RansacParams {
        max_iterations: 100,
        max_sigma: 0.33,
        min_inlier_ratio: 0.5,
        ..Default::default()
    });
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

    let config = RansacParams {
        seed: Some(12345),
        ..Default::default()
    };

    let estimator1 = RansacEstimator::new(config.clone());
    let result1 = estimate_uniform(
        &estimator1,
        &ref_points,
        &target_points,
        TransformType::Translation,
    )
    .unwrap();

    let estimator2 = RansacEstimator::new(config);
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

    let estimator = RansacEstimator::new(RansacParams {
        max_iterations: 10000,
        max_sigma: 0.33,
        confidence: 0.999,
        ..Default::default()
    });

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

    let estimator = RansacEstimator::new(RansacParams {
        max_iterations: 100,
        max_sigma: 0.33,
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
}

// ============================================================================
// RANSAC with different transform types
// ============================================================================

#[test]
fn test_ransac_affine() {
    let ref_points = make_grid(4, 2, 25.0);
    let known = Transform::affine([1.1, 0.2, 10.0, -0.1, 0.95, 5.0]);
    let target_points = apply_all(&known, &ref_points);

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
        ..Default::default()
    });
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.33,
        ..Default::default()
    });
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
        ..Default::default()
    });
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

// ============================================================================
// Numerical Stability Tests
// ============================================================================

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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 33.0,
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.033,
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
        ..Default::default()
    });
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 0.17,
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
    assert!(
        (result.transform.scale_factor() - tiny_scale).abs() < 0.0001,
        "Expected scale ~{}, got {}",
        tiny_scale,
        result.transform.scale_factor()
    );
}

// ============================================================================
// LO-RANSAC Tests
// ============================================================================

#[test]
fn test_lo_ransac_converges_to_exact_solution() {
    let ref_points = make_grid(4, 2, 10.0);
    let known = Transform::translation(DVec2::new(5.0, 3.0));
    let target_points = apply_all(&known, &ref_points);

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        use_local_optimization: true,
        lo_max_iterations: 10,
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

    let result_with_lo = RansacEstimator::new(RansacParams {
        seed: Some(123),
        use_local_optimization: true,
        lo_max_iterations: 5,
        max_sigma: 0.33,
        ..Default::default()
    })
    .estimate(
        &matches,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();

    let result_without_lo = RansacEstimator::new(RansacParams {
        seed: Some(123),
        use_local_optimization: false,
        max_sigma: 0.33,
        ..Default::default()
    })
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

// ============================================================================
// Progressive (confidence-weighted) RANSAC tests
// ============================================================================

#[test]
fn test_progressive_ransac_basic() {
    let ref_points = make_grid(5, 4, 20.0);
    let offset = DVec2::new(15.0, -8.0);
    let target_points: Vec<DVec2> = ref_points.iter().map(|p| *p + offset).collect();

    let confidences = vec![0.9; 20];
    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
        max_iterations: 200,
        max_sigma: 0.67,
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(999),
        max_iterations: 200,
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

// ============================================================================
// PointMatch-based estimation tests
// ============================================================================

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

    let ransac = RansacEstimator::new(RansacParams {
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

    let ransac = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_sigma: 1.67,
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

    assert!(!result.inliers.contains(&4), "Outlier should not be inlier");
    assert_eq!(result.inliers.len(), 4);

    let est = result.transform.translation_components();
    assert!((est.x - 50.0).abs() < 0.5);
    assert!((est.y - (-30.0)).abs() < 0.5);
}

// ============================================================================
// Plausibility Check Tests
// ============================================================================

#[test]
fn test_plausibility_rejects_large_rotation() {
    let ref_points = make_grid(5, 4, 50.0);
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 30.0_f64.to_radians(), 1.0);
    let target_points = apply_all(&known, &ref_points);

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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
    let config_base = RansacParams {
        seed: Some(42),
        max_rotation: Some(10.0_f64.to_radians()),
        scale_range: Some((0.8, 1.2)),
        ..Default::default()
    };

    // Rotation OK (5deg) but scale too large (1.5) -- should fail
    let known = Transform::similarity(DVec2::new(5.0, -3.0), 5.0_f64.to_radians(), 1.5);
    let target_points = apply_all(&known, &ref_points);
    let estimator = RansacEstimator::new(config_base.clone());
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
    let estimator = RansacEstimator::new(config_base.clone());
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
    let estimator = RansacEstimator::new(config_base);
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
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

    let estimator = RansacEstimator::new(RansacParams {
        seed: Some(42),
        max_rotation: Some(5.0_f64.to_radians()),
        scale_range: Some((0.9, 1.1)),
        max_sigma: 0.67,
        ..Default::default()
    });
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
