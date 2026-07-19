use crate::stacking::registration::triangle::tests::*;

#[test]
fn test_match_triangles_too_few_points() {
    let two = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let three = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
    ];

    // Both sides need >= 3 points
    assert!(match_triangles(&two, &three, &TriangleConfig::default()).is_empty());
    assert!(match_triangles(&three, &two, &TriangleConfig::default()).is_empty());
    assert!(match_triangles(&two, &two, &TriangleConfig::default()).is_empty());
}

#[test]
fn test_match_triangles_empty_inputs() {
    let empty: Vec<DVec2> = vec![];
    let three = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
    ];
    assert!(match_triangles(&empty, &three, &TriangleConfig::default()).is_empty());
    assert!(match_triangles(&three, &empty, &TriangleConfig::default()).is_empty());
}

#[test]
fn test_match_identical_star_lists() {
    // 5 points with asymmetric pattern → each matches itself
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let matches = match_triangles(&positions, &positions, &TriangleConfig::default());

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_match_translated_stars() {
    // Translation preserves triangle ratios → all 5 match
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let offset = DVec2::new(100.0, 50.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleConfig::default(),
    );

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_match_scaled_stars() {
    // Uniform scaling preserves triangle ratios → all 5 match
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p * 2.0).collect();

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleConfig::default(),
    );

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_match_rotated_stars() {
    // 90-degree rotation: (x,y) → (-y,x). Preserves ratios. Orientation check off
    // for symmetric pattern to avoid ambiguous correspondence.
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| DVec2::new(-p.y, p.x))
        .collect();

    let config = TriangleConfig {
        check_orientation: false,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);
    assert_eq!(matches.len(), 5);
}

#[test]
fn test_match_with_missing_stars() {
    // Target has 4 of 5 reference stars → should match exactly 4
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let target_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleConfig::default(),
    );

    assert_eq!(matches.len(), 4);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_match_with_extra_stars() {
    // Target has all 4 ref stars plus 2 extras → should match all 4 ref stars
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let target_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(15.0, 15.0),
    ];

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleConfig::default(),
    );

    assert_eq!(matches.len(), 4);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_match_mirrored_image_orientation_effect() {
    // Mirror flips orientation. With orientation check on, mirrored should get
    // fewer matches than with it off.
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| DVec2::new(-p.x, p.y))
        .collect();

    let config_with = TriangleConfig {
        check_orientation: true,
        min_votes: 1,
        ..Default::default()
    };
    let matches_with = match_triangles(&ref_positions, &target_positions, &config_with);

    let config_without = TriangleConfig {
        check_orientation: false,
        min_votes: 1,
        ..Default::default()
    };
    let matches_without = match_triangles(&ref_positions, &target_positions, &config_without);

    assert!(
        matches_without.len() >= matches_with.len(),
        "Orientation check should not increase matches: with={}, without={}",
        matches_with.len(),
        matches_without.len()
    );
}

#[test]
fn test_match_with_outliers() {
    // 6 real stars + 4 far-away outliers. Matches among real stars should be correct.
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(20.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(20.0, 10.0),
    ];

    let mut target_positions = ref_positions.clone();
    target_positions.push(DVec2::new(100.0, 100.0));
    target_positions.push(DVec2::new(150.0, 50.0));
    target_positions.push(DVec2::new(75.0, 125.0));
    target_positions.push(DVec2::new(200.0, 200.0));

    let config = TriangleConfig {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // Should match at least 4 of the 6 real stars
    assert!(
        matches.len() >= 4,
        "With outliers, found only {} matches",
        matches.len()
    );

    // All matches among the 6 real stars should be correct (same index)
    for m in &matches {
        if m.ref_idx < 6 && m.target_idx < 6 {
            assert_eq!(m.ref_idx, m.target_idx);
        }
    }
}

#[test]
fn test_match_permuted_indices() {
    // Same 5 geometric points, target in reversed order.
    // ref[i] corresponds to target[4-i]
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(30.0, 0.0),
        DVec2::new(15.0, 40.0),
        DVec2::new(50.0, 20.0),
        DVec2::new(10.0, 25.0),
    ];

    let target_points: Vec<DVec2> = points.iter().rev().copied().collect();

    let config = TriangleConfig {
        min_votes: 1,
        ..Default::default()
    };

    let matches = match_triangles(&points, &target_points, &config);

    assert!(
        matches.len() >= 4,
        "Should match most points, got {}",
        matches.len()
    );

    for m in &matches {
        let expected_target = 4 - m.ref_idx;
        assert_eq!(
            m.target_idx, expected_target,
            "ref {} should match target {} (same geometric point), got target {}",
            m.ref_idx, expected_target, m.target_idx
        );
    }
}

#[test]
fn test_match_ratio_tolerance_sensitivity() {
    // Different ratio_tolerance values should produce different match counts.
    // Tighter tolerance → fewer matches (or same), looser → more (or same).
    let ref_positions: Vec<DVec2> = (0..25)
        .map(|i| {
            let base_x = (i % 5) as f64 * 80.0 + 100.0;
            let base_y = (i / 5) as f64 * 80.0 + 100.0;
            // Deterministic jitter to break grid symmetry
            let jitter_x = ((i * 13 + 7) as f64 * 0.37).sin() * 15.0;
            let jitter_y = ((i * 17 + 3) as f64 * 0.53).cos() * 15.0;
            DVec2::new(base_x + jitter_x, base_y + jitter_y)
        })
        .collect();

    // Add sub-pixel noise to break exact ratio equality
    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let noise_x = ((i * 7 + 3) as f64 * 0.73).sin() * 0.3;
            let noise_y = ((i * 11 + 5) as f64 * 0.91).cos() * 0.3;
            DVec2::new(p.x + noise_x, p.y + noise_y)
        })
        .collect();

    let tight = TriangleConfig {
        ratio_tolerance: 0.001,
        min_votes: 2,
        ..Default::default()
    };
    let loose = TriangleConfig {
        ratio_tolerance: 0.1,
        min_votes: 2,
        ..Default::default()
    };

    let matches_tight = match_triangles(&ref_positions, &target_positions, &tight);
    let matches_loose = match_triangles(&ref_positions, &target_positions, &loose);

    // Loose tolerance should find at least as many matches
    assert!(
        matches_loose.len() >= matches_tight.len(),
        "Loose tolerance ({}) should find >= tight tolerance ({}) matches",
        matches_loose.len(),
        matches_tight.len()
    );
}

#[test]
fn test_match_min_votes_sensitivity() {
    // Higher min_votes should produce fewer (or equal) matches
    let ref_positions: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 50.0;
            let y = (i / 5) as f64 * 50.0;
            DVec2::new(x, y)
        })
        .collect();

    let target_positions = ref_positions.clone();

    let low_min = TriangleConfig {
        min_votes: 1,
        ..Default::default()
    };
    let high_min = TriangleConfig {
        min_votes: 5,
        ..Default::default()
    };

    let matches_low = match_triangles(&ref_positions, &target_positions, &low_min);
    let matches_high = match_triangles(&ref_positions, &target_positions, &high_min);

    assert!(
        matches_low.len() >= matches_high.len(),
        "min_votes=1 ({}) should find >= min_votes=5 ({}) matches",
        matches_low.len(),
        matches_high.len()
    );
}

#[test]
fn test_match_sparse_field_10_stars() {
    // 10 stars in a grid-like pattern with one off-grid point for asymmetry
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(50.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 50.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(100.0, 50.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 25.0), // Breaks grid symmetry
    ];

    let offset = DVec2::new(10.0, 20.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let config = TriangleConfig {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // Translation-invariant matching should find all 10 stars
    assert_eq!(matches.len(), 10);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_match_with_subpixel_noise() {
    // 25 irregular positions with +-0.3 pixel noise
    let ref_positions: Vec<DVec2> = (0..25)
        .map(|i| {
            let base_x = (i % 5) as f64 * 80.0 + 100.0;
            let base_y = (i / 5) as f64 * 80.0 + 100.0;
            let jitter_x = ((i * 13 + 7) as f64 * 0.37).sin() * 15.0;
            let jitter_y = ((i * 17 + 3) as f64 * 0.53).cos() * 15.0;
            DVec2::new(base_x + jitter_x, base_y + jitter_y)
        })
        .collect();

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let noise_x = ((i * 7 + 3) as f64 * 0.73).sin() * 0.3;
            let noise_y = ((i * 11 + 5) as f64 * 0.91).cos() * 0.3;
            DVec2::new(p.x + noise_x, p.y + noise_y)
        })
        .collect();

    let config = TriangleConfig::default();
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // With 80-pixel spacing and 0.3-pixel noise, ratios change by < 0.01 tolerance
    // Should match most of the 25 stars
    assert!(
        matches.len() >= 20,
        "Noisy matching found only {} matches",
        matches.len()
    );

    // All matches should be correct (noise is small relative to spacing)
    for m in &matches {
        assert_eq!(
            m.ref_idx, m.target_idx,
            "Incorrect noisy match: ref {} != target {}",
            m.ref_idx, m.target_idx
        );
    }
}

#[test]
fn test_triangle_similarity_threshold_boundary() {
    // Equilateral: all sides equal → ratios = (1.0, 1.0)
    let tri1 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(5.0, 8.66),
        ],
    )
    .unwrap();

    // Distorted: slightly changed apex
    let tri2 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(4.5, 8.0),
        ],
    )
    .unwrap();

    let dr0 = (tri1.ratios.0 - tri2.ratios.0).abs();
    let dr1 = (tri1.ratios.1 - tri2.ratios.1).abs();

    // Large tolerance: should match
    assert!(
        tri1.is_similar(&tri2, 0.15),
        "Should be similar at 15% tol (dr0={dr0:.4}, dr1={dr1:.4})"
    );

    // Tight tolerance: should not match
    assert!(
        !tri1.is_similar(&tri2, 0.01),
        "Should not be similar at 1% tol (dr0={dr0:.4}, dr1={dr1:.4})"
    );
}
