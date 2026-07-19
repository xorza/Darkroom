use crate::stacking::registration::ransac::tests::*;

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

#[test]
fn test_random_sample_into_produces_unique_indices() {
    use rand::SeedableRng;
    let mut rng = SmallRng::seed_from_u64(42);
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
    let mut rng = SmallRng::seed_from_u64(99);
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

#[test]
fn test_weighted_sample_into_pool_smaller_than_k() {
    // When pool.len() <= k, should return all pool elements
    use rand::SeedableRng;
    let mut rng = SmallRng::seed_from_u64(42);
    let pool = vec![5, 10, 15];
    let weights = vec![0.0; 20]; // weights indexed by pool values
    let mut buffer = Vec::new();
    let mut scratch = Vec::new();

    weighted_sample_into(&mut rng, &pool, &weights, 5, &mut buffer, &mut scratch);
    let mut sorted = buffer.clone();
    sorted.sort();
    assert_eq!(sorted, vec![5, 10, 15]);
}

#[test]
fn test_weighted_sample_into_returns_k_unique() {
    use rand::SeedableRng;
    let mut rng = SmallRng::seed_from_u64(42);
    let pool: Vec<usize> = (0..20).collect();
    let weights: Vec<f64> = (0..20).map(|i| i as f64 + 1.0).collect();
    let k = 4;
    let mut buffer = Vec::new();
    let mut scratch = Vec::new();

    for _ in 0..100 {
        weighted_sample_into(&mut rng, &pool, &weights, k, &mut buffer, &mut scratch);
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
