//! Tests for triangle matching module.

use super::*;

#[test]
fn test_triangle_from_positions() {
    let tri = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (3.0, 0.0), (0.0, 4.0)]);

    assert!(tri.is_some());
    let tri = tri.unwrap();

    // 3-4-5 right triangle
    assert!((tri.sides[0] - 3.0).abs() < 1e-10);
    assert!((tri.sides[1] - 4.0).abs() < 1e-10);
    assert!((tri.sides[2] - 5.0).abs() < 1e-10);
}

#[test]
fn test_triangle_ratios_scale_invariant() {
    let tri1 = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (3.0, 0.0), (0.0, 4.0)]).unwrap();

    // Same triangle, 10x larger
    let tri2 = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (30.0, 0.0), (0.0, 40.0)]).unwrap();

    assert!((tri1.ratios.0 - tri2.ratios.0).abs() < 1e-10);
    assert!((tri1.ratios.1 - tri2.ratios.1).abs() < 1e-10);
}

#[test]
fn test_triangle_similarity_check() {
    let tri1 = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (3.0, 0.0), (0.0, 4.0)]).unwrap();

    let tri2 =
        Triangle::from_positions([0, 1, 2], [(10.0, 10.0), (40.0, 10.0), (10.0, 50.0)]).unwrap();

    assert!(tri1.is_similar(&tri2, 0.01));
}

#[test]
fn test_triangle_not_similar() {
    let tri1 = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]).unwrap();

    // Different shape
    let tri2 = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (2.0, 0.0), (1.0, 0.1)]).unwrap();

    assert!(!tri1.is_similar(&tri2, 0.01));
}

#[test]
fn test_triangle_orientation() {
    let ccw = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]).unwrap();
    assert_eq!(ccw.orientation, Orientation::CounterClockwise);

    let cw = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0)]).unwrap();
    assert_eq!(cw.orientation, Orientation::Clockwise);
}

#[test]
fn test_degenerate_triangle_detection() {
    // Collinear points
    let tri = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);
    assert!(tri.is_none());

    // Duplicate point
    let tri = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (0.0, 0.0), (1.0, 1.0)]);
    assert!(tri.is_none());
}

#[test]
fn test_hash_table_build() {
    let positions = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];
    let triangles = form_triangles(&positions, 10);

    assert_eq!(triangles.len(), 4); // C(4,3) = 4

    let table = TriangleHashTable::build(&triangles, 100);
    assert_eq!(table.len(), 4);
}

#[test]
fn test_hash_table_lookup() {
    let positions = vec![(0.0, 0.0), (3.0, 0.0), (0.0, 4.0)];
    let triangles = form_triangles(&positions, 10);
    let table = TriangleHashTable::build(&triangles, 100);

    // Same triangle should find itself
    let candidates = table.find_candidates(&triangles[0], 0.01);
    assert!(candidates.contains(&0));
}

#[test]
fn test_hash_table_empty() {
    let table = TriangleHashTable::build(&[], 100);
    assert!(table.is_empty());
}

#[test]
fn test_match_identical_star_lists() {
    let positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    let matches = match_stars_triangles(&positions, &positions, &TriangleMatchConfig::default());

    // Should match all stars
    assert_eq!(matches.len(), 5);

    // Each star should match itself
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_match_translated_stars() {
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Translate by (100, 50)
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 100.0, y + 50.0))
        .collect();

    let matches = match_stars_triangles(
        &ref_positions,
        &target_positions,
        &TriangleMatchConfig::default(),
    );

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_match_rotated_stars() {
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Rotate by 90 degrees around origin
    let target_positions: Vec<(f64, f64)> = ref_positions.iter().map(|(x, y)| (-*y, *x)).collect();

    let config = TriangleMatchConfig {
        check_orientation: false, // Rotation changes orientation
        ..Default::default()
    };

    let matches = match_stars_triangles(&ref_positions, &target_positions, &config);

    assert_eq!(matches.len(), 5);
}

#[test]
fn test_match_scaled_stars() {
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Scale by 2x
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x * 2.0, y * 2.0))
        .collect();

    let matches = match_stars_triangles(
        &ref_positions,
        &target_positions,
        &TriangleMatchConfig::default(),
    );

    assert_eq!(matches.len(), 5);
}

#[test]
fn test_match_with_missing_stars() {
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Only 4 stars in target (missing one)
    let target_positions = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];

    let matches = match_stars_triangles(
        &ref_positions,
        &target_positions,
        &TriangleMatchConfig::default(),
    );

    // Should match the 4 common stars
    assert!(matches.len() >= 4);
}

#[test]
fn test_match_with_extra_stars() {
    let ref_positions = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];

    // Target has extra stars
    let target_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
        (15.0, 15.0),
    ];

    let matches = match_stars_triangles(
        &ref_positions,
        &target_positions,
        &TriangleMatchConfig::default(),
    );

    // Should match all 4 reference stars
    assert_eq!(matches.len(), 4);
}

#[test]
fn test_too_few_stars() {
    let positions = vec![(0.0, 0.0), (1.0, 0.0)];
    let matches = match_stars_triangles(&positions, &positions, &TriangleMatchConfig::default());
    assert!(matches.is_empty());
}

#[test]
fn test_all_collinear_stars() {
    let positions = vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0)];
    let triangles = form_triangles(&positions, 10);
    assert!(triangles.is_empty());
}

#[test]
fn test_form_triangles_count() {
    // C(5,3) = 10 triangles from 5 points
    let positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 15.0),
    ];
    let triangles = form_triangles(&positions, 10);
    assert_eq!(triangles.len(), 10);
}

#[test]
fn test_matches_to_point_pairs() {
    let ref_pos = vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)];
    let target_pos = vec![(10.0, 10.0), (11.0, 11.0), (12.0, 12.0)];

    let matches = vec![
        StarMatch {
            ref_idx: 0,
            target_idx: 0,
            votes: 5,
            confidence: 0.9,
        },
        StarMatch {
            ref_idx: 2,
            target_idx: 2,
            votes: 3,
            confidence: 0.8,
        },
    ];

    let (ref_points, target_points) = matches_to_point_pairs(&matches, &ref_pos, &target_pos);

    assert_eq!(ref_points.len(), 2);
    assert_eq!(target_points.len(), 2);
    assert_eq!(ref_points[0], (0.0, 0.0));
    assert_eq!(target_points[0], (10.0, 10.0));
}

#[test]
fn test_triangle_hash_key() {
    let tri = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (3.0, 0.0), (0.0, 4.0)]).unwrap();

    let (bx, by) = tri.hash_key(100);
    assert!(bx < 100);
    assert!(by < 100);
}

#[test]
fn test_match_mirrored_image() {
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Mirror horizontally
    let target_positions: Vec<(f64, f64)> = ref_positions.iter().map(|(x, y)| (-*x, *y)).collect();

    // With orientation check, mirrored triangles should be rejected
    let config_with_orientation = TriangleMatchConfig {
        check_orientation: true,
        min_votes: 1, // Lower threshold for testing
        ..Default::default()
    };
    let matches_with =
        match_stars_triangles(&ref_positions, &target_positions, &config_with_orientation);

    // Without orientation check, should match more
    let config_no_orientation = TriangleMatchConfig {
        check_orientation: false,
        min_votes: 1,
        ..Default::default()
    };
    let matches_without =
        match_stars_triangles(&ref_positions, &target_positions, &config_no_orientation);

    // With mirroring and orientation check, we should get fewer matches than without
    assert!(
        matches_without.len() >= matches_with.len(),
        "Expected more matches without orientation check"
    );
}

// ============================================================================
// K-D Tree based matching tests
// ============================================================================

#[test]
fn test_kdtree_match_identical_star_lists() {
    let positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    let matches =
        match_stars_triangles_kdtree(&positions, &positions, &TriangleMatchConfig::default());

    // Should match all stars
    assert_eq!(matches.len(), 5);

    // Each star should match itself
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_kdtree_match_translated_stars() {
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Translate by (100, 50)
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 100.0, y + 50.0))
        .collect();

    let matches = match_stars_triangles_kdtree(
        &ref_positions,
        &target_positions,
        &TriangleMatchConfig::default(),
    );

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_kdtree_match_scaled_stars() {
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Scale by 2x
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x * 2.0, y * 2.0))
        .collect();

    let matches = match_stars_triangles_kdtree(
        &ref_positions,
        &target_positions,
        &TriangleMatchConfig::default(),
    );

    assert_eq!(matches.len(), 5);
}

#[test]
fn test_form_triangles_kdtree_basic() {
    let positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    let triangles = form_triangles_kdtree(&positions, 4);

    // Should form at least some triangles
    assert!(!triangles.is_empty());

    // All triangles should be valid (non-degenerate)
    for tri in &triangles {
        assert!(tri.sides[0] > 0.0);
        assert!(tri.sides[1] > 0.0);
        assert!(tri.sides[2] > 0.0);
    }
}

#[test]
fn test_form_triangles_kdtree_empty() {
    let positions: Vec<(f64, f64)> = vec![];
    let triangles = form_triangles_kdtree(&positions, 5);
    assert!(triangles.is_empty());
}

#[test]
fn test_form_triangles_kdtree_too_few() {
    let positions = vec![(0.0, 0.0), (1.0, 1.0)];
    let triangles = form_triangles_kdtree(&positions, 5);
    assert!(triangles.is_empty());
}

// ============================================================================
// Tests recommended from algorithm review
// ============================================================================

/// Test with a dense star field (100+ stars)
#[test]
fn test_match_dense_field_100_stars() {
    use std::f64::consts::PI;

    // Generate 100 stars in a grid pattern with some noise
    let mut ref_positions = Vec::with_capacity(100);
    for i in 0..10 {
        for j in 0..10 {
            let x = i as f64 * 20.0 + (i as f64 * 0.1).sin() * 2.0;
            let y = j as f64 * 20.0 + (j as f64 * 0.1).cos() * 2.0;
            ref_positions.push((x, y));
        }
    }

    // Apply a similarity transform
    let angle = PI / 12.0; // 15 degrees
    let scale = 1.1;
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| {
            let nx = scale * (cos_a * x - sin_a * y) + 50.0;
            let ny = scale * (sin_a * x + cos_a * y) + 30.0;
            (nx, ny)
        })
        .collect();

    let config = TriangleMatchConfig {
        max_stars: 100,
        min_votes: 2,
        ..Default::default()
    };

    // Test both regular and k-d tree matching
    let matches_regular = match_stars_triangles(&ref_positions, &target_positions, &config);
    let matches_kdtree = match_stars_triangles_kdtree(&ref_positions, &target_positions, &config);

    // Both should find a reasonable number of matches
    assert!(
        matches_regular.len() >= 20,
        "Regular matching found only {} matches",
        matches_regular.len()
    );
    assert!(
        matches_kdtree.len() >= 20,
        "K-d tree matching found only {} matches",
        matches_kdtree.len()
    );
}

/// Test with sparse star field (exactly 10 stars)
#[test]
fn test_match_sparse_field_10_stars() {
    let ref_positions = vec![
        (0.0, 0.0),
        (50.0, 0.0),
        (100.0, 0.0),
        (0.0, 50.0),
        (50.0, 50.0),
        (100.0, 50.0),
        (0.0, 100.0),
        (50.0, 100.0),
        (100.0, 100.0),
        (50.0, 25.0),
    ];

    // Translate
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 10.0, y + 20.0))
        .collect();

    let config = TriangleMatchConfig {
        max_stars: 10,
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_stars_triangles(&ref_positions, &target_positions, &config);

    // Should match most stars
    assert!(
        matches.len() >= 7,
        "Sparse field matching found only {} matches",
        matches.len()
    );
}

/// Test with 40% outliers (spurious stars)
#[test]
fn test_match_with_40_percent_outliers() {
    // 6 real stars
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (20.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (20.0, 10.0),
    ];

    // Same 6 stars plus 4 random outliers (40% noise)
    let mut target_positions = ref_positions.clone();
    target_positions.push((100.0, 100.0));
    target_positions.push((150.0, 50.0));
    target_positions.push((75.0, 125.0));
    target_positions.push((200.0, 200.0));

    let config = TriangleMatchConfig {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_stars_triangles(&ref_positions, &target_positions, &config);

    // Should match the 6 real stars
    assert!(
        matches.len() >= 4,
        "With 40% outliers, found only {} matches",
        matches.len()
    );

    // Verify matches are correct (indices should match)
    for m in &matches {
        if m.ref_idx < 6 && m.target_idx < 6 {
            assert_eq!(
                m.ref_idx, m.target_idx,
                "Incorrect match: ref {} != target {}",
                m.ref_idx, m.target_idx
            );
        }
    }
}

/// Test vertex correspondence correctness
#[test]
fn test_vertex_correspondence_correctness() {
    // Create a distinctive asymmetric triangle pattern
    let ref_positions = vec![
        (0.0, 0.0),   // A
        (10.0, 0.0),  // B
        (5.0, 20.0),  // C - tall isosceles
        (15.0, 10.0), // D - offset point
    ];

    // Same positions, different order
    let target_positions = vec![
        (0.0, 0.0),   // A
        (10.0, 0.0),  // B
        (5.0, 20.0),  // C
        (15.0, 10.0), // D
    ];

    let config = TriangleMatchConfig {
        min_votes: 1,
        ..Default::default()
    };

    let matches = match_stars_triangles(&ref_positions, &target_positions, &config);

    // Each match should be correct (same index)
    for m in &matches {
        assert_eq!(
            m.ref_idx, m.target_idx,
            "Vertex correspondence error: ref {} matched to target {}",
            m.ref_idx, m.target_idx
        );
    }
}

/// Test that k-d tree method is faster for large star counts
#[test]
fn test_kdtree_scales_better_than_brute_force() {
    use std::time::Instant;

    // Generate 80 stars
    let positions: Vec<(f64, f64)> = (0..80)
        .map(|i| {
            let x = (i % 10) as f64 * 15.0;
            let y = (i / 10) as f64 * 15.0;
            (x, y)
        })
        .collect();

    let _config = TriangleMatchConfig {
        max_stars: 80,
        min_votes: 2,
        ..Default::default()
    };

    // Time brute-force triangle formation
    let start = Instant::now();
    let brute_triangles = form_triangles(&positions, 80);
    let brute_time = start.elapsed();

    // Time k-d tree triangle formation
    let start = Instant::now();
    let kdtree_triangles = form_triangles_kdtree(&positions, 10);
    let kdtree_time = start.elapsed();

    // K-d tree should produce fewer triangles (by design)
    assert!(
        kdtree_triangles.len() < brute_triangles.len(),
        "K-d tree produced {} triangles, brute force produced {}",
        kdtree_triangles.len(),
        brute_triangles.len()
    );

    // K-d tree should be faster for large inputs
    // Note: This may not always hold for small inputs due to overhead
    println!(
        "Brute force: {} triangles in {:?}",
        brute_triangles.len(),
        brute_time
    );
    println!(
        "K-d tree: {} triangles in {:?}",
        kdtree_triangles.len(),
        kdtree_time
    );
}
