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

// ============================================================================
// Additional edge case tests
// ============================================================================

/// Test quadrant consistency with genuinely inconsistent data
#[test]
fn test_quadrant_consistency_truly_inconsistent() {
    // Points spread across all quadrants in reference
    let ref_points = vec![
        (10.0, 10.0), // TL
        (90.0, 10.0), // TR
        (10.0, 90.0), // BL
        (90.0, 90.0), // BR
        (50.0, 10.0), // T
        (50.0, 90.0), // B
        (10.0, 50.0), // L
        (90.0, 50.0), // R
    ];

    // Target: apply consistent transform to all except BR quadrant
    let mut target_points = Vec::new();
    for &(x, y) in &ref_points {
        if x > 50.0 && y > 50.0 {
            // BR quadrant - apply different (wrong) transform
            target_points.push((x + 20.0, y + 20.0));
        } else {
            // Other quadrants - apply correct transform
            target_points.push((x + 5.0, y + 5.0));
        }
    }

    let transform = TransformMatrix::from_translation(5.0, 5.0);
    let consistency = check_quadrant_consistency(&ref_points, &target_points, &transform, 100, 100);

    // BR quadrant (index 3) should have much higher error
    assert!(
        consistency.quadrant_rms[3] > consistency.quadrant_rms[0] * 5.0,
        "BR quadrant should have ~5x higher error: {:?}",
        consistency.quadrant_rms
    );
}

/// Test overlap estimation with 0% overlap (complete disjoint)
#[test]
fn test_estimate_overlap_complete_disjoint() {
    // Large translation way beyond image boundaries
    let transform = TransformMatrix::from_translation(1000.0, 1000.0);
    let overlap = estimate_overlap(100, 100, &transform);
    assert!(
        overlap < 0.01,
        "Complete disjoint should have ~0% overlap, got {}",
        overlap
    );
}

/// Test overlap estimation with 100% overlap (identity)
#[test]
fn test_estimate_overlap_perfect() {
    let transform = TransformMatrix::identity();
    let overlap = estimate_overlap(100, 100, &transform);
    assert!(
        (overlap - 1.0).abs() < 0.01,
        "Identity should have 100% overlap, got {}",
        overlap
    );
}

/// Test overlap with negative coordinate shift
#[test]
fn test_estimate_overlap_negative_coords() {
    let transform = TransformMatrix::from_translation(-30.0, -30.0);
    let overlap = estimate_overlap(100, 100, &transform);
    // 70% overlap in each dimension -> ~49% overlap area
    assert!(
        overlap > 0.3 && overlap < 0.7,
        "Negative shift should have partial overlap, got {}",
        overlap
    );
}

/// Test overlap with rotation - rotation around center preserves corners
#[test]
fn test_estimate_overlap_rotation() {
    // Small rotation around center - corners stay in bounds
    let angle = 0.1; // ~6 degrees
    let transform = TransformMatrix::from_rotation_around(angle, 50.0, 50.0);
    let overlap = estimate_overlap(100, 100, &transform);

    // Small rotation should have high overlap (corners stay mostly in bounds)
    assert!(
        overlap > 0.8,
        "Small rotation should have high overlap, got {}",
        overlap
    );

    // 90 degree rotation around center - all corners still inside
    let angle_90 = std::f64::consts::PI / 2.0;
    let transform_90 = TransformMatrix::from_rotation_around(angle_90, 50.0, 50.0);
    let overlap_90 = estimate_overlap(100, 100, &transform_90);
    assert!(
        overlap_90 > 0.9,
        "90-degree rotation around center should preserve overlap, got {}",
        overlap_90
    );
}

/// Test quality score boundary values
#[test]
fn test_quality_score_boundaries() {
    // Very few inliers - should have low quality
    let few_residuals = vec![0.1, 0.1, 0.1];
    let metrics = QualityMetrics::compute(&few_residuals, 3, 3, 1.0);
    assert!(
        !metrics.is_valid,
        "Too few inliers should be invalid (need >= 4)"
    );

    // Exactly 4 inliers with low error - minimum valid
    let min_residuals = vec![0.1, 0.1, 0.1, 0.1];
    let metrics = QualityMetrics::compute(&min_residuals, 4, 4, 1.0);
    assert!(metrics.is_valid, "4 inliers with low error should be valid");

    // High RMS error
    let high_error = vec![10.0, 10.0, 10.0, 10.0, 10.0];
    let metrics = QualityMetrics::compute(&high_error, 5, 5, 1.0);
    assert!(!metrics.is_valid, "High RMS error should be invalid");
}

/// Test ResidualStats with single element
#[test]
fn test_residual_stats_single_element() {
    let residuals = vec![5.0];
    let stats = ResidualStats::compute(&residuals);

    assert!((stats.mean - 5.0).abs() < 0.01);
    assert!((stats.median - 5.0).abs() < 0.01);
    assert!((stats.min - 5.0).abs() < 0.01);
    assert!((stats.max - 5.0).abs() < 0.01);
    assert!((stats.rms - 5.0).abs() < 0.01);
    // Std dev of single element is undefined/0
    assert!(stats.std_dev < 0.01);
}

/// Test ResidualStats with identical values
#[test]
fn test_residual_stats_identical_values() {
    let residuals = vec![3.0, 3.0, 3.0, 3.0, 3.0];
    let stats = ResidualStats::compute(&residuals);

    assert!((stats.mean - 3.0).abs() < 0.01);
    assert!(
        (stats.std_dev).abs() < 0.01,
        "Std dev of identical values should be 0"
    );
    assert!((stats.percentile_90 - 3.0).abs() < 0.01);
}

/// Test compute_residuals with large transformation
#[test]
fn test_compute_residuals_large_transform() {
    let ref_points = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0)];
    let target_points = vec![(50.0, 50.0), (150.0, 50.0), (50.0, 150.0)];
    let transform = TransformMatrix::from_translation(50.0, 50.0);

    let residuals = compute_residuals(&ref_points, &target_points, &transform);

    assert_eq!(residuals.len(), 3);
    for r in &residuals {
        assert!(*r < 0.01, "Expected near-zero residual, got {}", r);
    }
}

/// Test quality metrics with very high inlier count
#[test]
fn test_quality_metrics_many_inliers() {
    // 100 inliers with very low error
    let residuals: Vec<f64> = (0..100).map(|_| 0.1).collect();
    let metrics = QualityMetrics::compute(&residuals, 100, 100, 1.0);

    assert!(metrics.is_valid);
    assert!(
        metrics.quality_score > 0.8,
        "Many good inliers should have high quality score, got {}",
        metrics.quality_score
    );
}

/// Test percentile edge cases
#[test]
fn test_percentile_edge_cases() {
    let sorted = vec![1.0, 2.0, 3.0];

    // 0th percentile should be minimum
    assert!((percentile(&sorted, 0.0) - 1.0).abs() < 0.1);

    // 100th percentile should be maximum
    assert!((percentile(&sorted, 1.0) - 3.0).abs() < 0.1);
}
