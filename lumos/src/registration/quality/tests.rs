use super::*;

#[test]
fn test_quality_metrics_perfect() {
    // 10 inliers with very low error and full overlap
    let residuals = vec![0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1];
    let metrics = QualityMetrics::compute(&residuals, 10, 10, 1.0);

    assert!(metrics.rms_error < 0.2);
    assert!(metrics.is_valid);
    assert!(metrics.quality_score > 0.5); // Reasonable threshold
}

#[test]
fn test_quality_metrics_high_error() {
    let residuals = vec![10.0, 8.0, 12.0, 9.0, 11.0];
    let metrics = QualityMetrics::compute(&residuals, 5, 5, 1.0);

    assert!(metrics.rms_error > 5.0);
    assert!(!metrics.is_valid);
    assert!(metrics.quality_score < 0.5);
}

#[test]
fn test_quality_metrics_low_inlier_ratio() {
    // 5 inliers out of 20 matches = 0.25 ratio < 0.3 threshold
    let residuals = vec![0.5, 0.5, 0.5, 0.5, 0.5];
    let metrics = QualityMetrics::compute(&residuals, 20, 5, 1.0);

    assert!(!metrics.is_valid);
    assert!(metrics.failure_reason.unwrap().contains("inlier ratio"));
}

#[test]
fn test_estimate_overlap_identity() {
    let transform = TransformMatrix::identity();
    let overlap = estimate_overlap(100, 100, &transform);
    assert!((overlap - 1.0).abs() < 0.01);
}

#[test]
fn test_estimate_overlap_translation() {
    let transform = TransformMatrix::from_translation(50.0, 0.0);
    let overlap = estimate_overlap(100, 100, &transform);
    // 50% horizontal shift -> ~50% overlap
    assert!((overlap - 0.5).abs() < 0.1);
}

#[test]
fn test_estimate_overlap_no_overlap() {
    let transform = TransformMatrix::from_translation(200.0, 0.0);
    let overlap = estimate_overlap(100, 100, &transform);
    assert!(overlap < 0.01);
}

#[test]
fn test_compute_residuals() {
    let ref_points = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
    let target_points = vec![(5.0, 0.0), (15.0, 0.0), (5.0, 10.0)];
    let transform = TransformMatrix::from_translation(5.0, 0.0);

    let residuals = compute_residuals(&ref_points, &target_points, &transform);

    assert_eq!(residuals.len(), 3);
    for r in &residuals {
        assert!(*r < 0.01, "Expected near-zero residual, got {}", r);
    }
}

#[test]
fn test_residual_stats() {
    let residuals = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let stats = ResidualStats::compute(&residuals);

    assert!((stats.mean - 3.0).abs() < 0.01);
    assert!((stats.median - 3.0).abs() < 0.01);
    assert!((stats.min - 1.0).abs() < 0.01);
    assert!((stats.max - 5.0).abs() < 0.01);
}

#[test]
fn test_residual_stats_empty() {
    let residuals: Vec<f64> = vec![];
    let stats = ResidualStats::compute(&residuals);

    assert_eq!(stats.mean, 0.0);
    assert_eq!(stats.rms, 0.0);
}

#[test]
fn test_quadrant_consistency_uniform() {
    let ref_points = vec![
        (25.0, 25.0), // TL
        (75.0, 25.0), // TR
        (25.0, 75.0), // BL
        (75.0, 75.0), // BR
    ];
    let target_points = ref_points.clone();
    let transform = TransformMatrix::identity();

    let consistency = check_quadrant_consistency(&ref_points, &target_points, &transform, 100, 100);

    assert!(consistency.is_consistent);
    assert_eq!(consistency.quadrant_counts, [1, 1, 1, 1]);
}

#[test]
fn test_quadrant_consistency_non_uniform() {
    let ref_points = vec![
        (25.0, 25.0), // TL - good
        (75.0, 25.0), // TR - good
        (25.0, 75.0), // BL - good
        (75.0, 75.0), // BR - bad
    ];
    let mut target_points = ref_points.clone();
    target_points[3] = (85.0, 85.0); // Shift BR point

    let transform = TransformMatrix::identity();

    let consistency = check_quadrant_consistency(&ref_points, &target_points, &transform, 100, 100);

    // BR quadrant should have higher error
    assert!(consistency.quadrant_rms[3] > consistency.quadrant_rms[0]);
}

#[test]
fn test_percentile() {
    let sorted = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    // 0.5 * 9 = 4.5, rounded to 5 -> sorted[5] = 6.0
    assert!((percentile(&sorted, 0.5) - 6.0).abs() < 0.1);
    // 0.9 * 9 = 8.1, rounded to 8 -> sorted[8] = 9.0
    assert!((percentile(&sorted, 0.9) - 9.0).abs() < 0.1);
}
