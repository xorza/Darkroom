use super::*;

#[test]
fn score_hypothesis_perfect_match() {
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
fn score_hypothesis_with_one_outlier() {
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
fn score_hypothesis_early_exit() {
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
