use crate::stacking::registration::ransac::tests::*;

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

#[test]
fn test_normalize_points_empty() {
    let normalization = point_normalization(&[]);
    // Transform should be identity
    let p = normalization.transform.apply(DVec2::new(1.0, 2.0));
    assert!((p.x - 1.0).abs() < TOL);
    assert!((p.y - 2.0).abs() < TOL);
}

#[test]
fn test_normalize_points_coincident() {
    // All points at same location: avg_dist=0 → returns identity transform
    let points = vec![DVec2::new(5.0, 5.0); 4];
    let normalization = point_normalization(&points);
    // Should return original points unchanged
    for &point in &points {
        let p = normalization.apply(point);
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
    let normalization = point_normalization(&points);
    let normalized: Vec<_> = points
        .iter()
        .map(|&point| normalization.apply(point))
        .collect();

    // Centroid of normalized points should be at origin
    let c = centroid(&normalized);
    assert!(c.x.abs() < TOL);
    assert!(c.y.abs() < TOL);

    // Average distance should be sqrt(2)
    let avg_dist: f64 =
        normalized.iter().map(|p| p.length()).sum::<f64>() / normalized.len() as f64;
    assert!((avg_dist - SQRT_2).abs() < 0.001);

    // Check exact normalized coordinates
    assert!((normalized[0].x - (-1.0)).abs() < TOL); // (0-5)*0.2 = -1
    assert!((normalized[0].y - (-1.0)).abs() < TOL);
    assert!((normalized[1].x - 1.0).abs() < TOL); // (10-5)*0.2 = 1
    assert!((normalized[1].y - (-1.0)).abs() < TOL);

    // Inverse transform should recover original points
    let inv = normalization.transform.inverse();
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

    let normalization = point_normalization(&points);
    let normalized: Vec<_> = points
        .iter()
        .map(|&point| normalization.apply(point))
        .collect();

    // Centroid at origin
    let c = centroid(&normalized);
    assert!(c.x.abs() < 1e-10, "Centroid x not at origin: {}", c.x);
    assert!(c.y.abs() < 1e-10, "Centroid y not at origin: {}", c.y);

    // Inverse recovers original (may lose some precision at 1e10 scale)
    let inv = normalization.transform.inverse();
    for (orig, norm) in points.iter().zip(normalized.iter()) {
        let r = inv.apply(*norm);
        assert!((r.x - orig.x).abs() < 1e-3, "X: {} vs {}", r.x, orig.x);
        assert!((r.y - orig.y).abs() < 1e-3, "Y: {} vs {}", r.y, orig.y);
    }
}

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
