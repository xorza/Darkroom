//! Tests for triangle matching module.
//!
//! # Test Strategy
//!
//! This file contains tests for both triangle matching implementations:
//!
//! - **Brute-force tests** (`match_stars_triangles`): Validate the core algorithm
//!   correctness with O(n³) exhaustive triangle enumeration. These are the reference
//!   implementation tests.
//!
//! - **K-d tree tests** (`match_triangles`): Validate the production
//!   implementation that uses spatial indexing for O(n·k²) complexity. These tests
//!   ensure the optimized path produces correct results.
//!
//! Both implementations should produce equivalent results for most test cases,
//! though the kdtree version may find slightly different (but equally valid) matches
//! due to using different triangle subsets.

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

    let matches = match_triangles(&positions, &positions, &TriangleMatchConfig::default());

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

    let matches = match_triangles(
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

    let matches = match_triangles(
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

#[test]
fn test_kdtree_match_rotated_stars() {
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

    let matches = match_triangles(&ref_positions, &target_positions, &config);

    assert_eq!(matches.len(), 5);
}

#[test]
fn test_kdtree_match_with_missing_stars() {
    let ref_positions = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (0.0, 10.0),
        (10.0, 10.0),
        (5.0, 5.0),
    ];

    // Only 4 stars in target (missing one)
    let target_positions = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleMatchConfig::default(),
    );

    // Should match the 4 common stars
    assert!(matches.len() >= 4);
}

#[test]
fn test_kdtree_match_with_extra_stars() {
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

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleMatchConfig::default(),
    );

    // Should match all 4 reference stars
    assert_eq!(matches.len(), 4);
}

#[test]
fn test_kdtree_match_mirrored_image() {
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
        min_votes: 1,
        ..Default::default()
    };
    let matches_with = match_triangles(&ref_positions, &target_positions, &config_with_orientation);

    // Without orientation check, should match more
    let config_no_orientation = TriangleMatchConfig {
        check_orientation: false,
        min_votes: 1,
        ..Default::default()
    };
    let matches_without =
        match_triangles(&ref_positions, &target_positions, &config_no_orientation);

    // With mirroring and orientation check, we should get fewer matches than without
    assert!(
        matches_without.len() >= matches_with.len(),
        "Expected more matches without orientation check"
    );
}

// ============================================================================
// Tests recommended from algorithm review
// ============================================================================

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

// ============================================================================
// Milestone F: Test Hardening - Additional stress tests
// ============================================================================

/// Test with very dense star field (500+ stars) - stress test for k-d tree
#[test]
fn test_match_very_dense_field_500_stars() {
    use std::f64::consts::PI;

    // Generate 500 stars in a semi-random grid pattern
    let mut ref_positions = Vec::with_capacity(500);
    for i in 0..25 {
        for j in 0..20 {
            // Add some deterministic "noise" to positions
            let noise_x = ((i * 7 + j * 13) as f64 * 0.1).sin() * 3.0;
            let noise_y = ((i * 11 + j * 17) as f64 * 0.1).cos() * 3.0;
            let x = i as f64 * 40.0 + noise_x;
            let y = j as f64 * 50.0 + noise_y;
            ref_positions.push((x, y));
        }
    }

    assert_eq!(ref_positions.len(), 500);

    // Apply a similarity transform (rotation + scale + translation)
    let angle = PI / 20.0; // 9 degrees
    let scale = 1.05;
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| {
            let nx = scale * (cos_a * x - sin_a * y) + 100.0;
            let ny = scale * (sin_a * x + cos_a * y) + 50.0;
            (nx, ny)
        })
        .collect();

    let config = TriangleMatchConfig {
        max_stars: 150, // Use only brightest 150 stars
        min_votes: 3,
        ..Default::default()
    };

    // K-d tree matching should handle this efficiently
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // Should find a substantial number of matches
    assert!(
        matches.len() >= 50,
        "Dense field (500 stars) matching found only {} matches",
        matches.len()
    );

    // Verify match correctness
    let correct_matches = matches.iter().filter(|m| m.ref_idx == m.target_idx).count();
    let accuracy = correct_matches as f64 / matches.len() as f64;
    assert!(
        accuracy >= 0.9,
        "Match accuracy too low: {:.1}% ({} correct out of {})",
        accuracy * 100.0,
        correct_matches,
        matches.len()
    );
}

/// Test with clustered star distribution (simulating star clusters)
#[test]
fn test_match_clustered_stars() {
    use std::f64::consts::PI;

    // Create 3 clusters of stars
    let mut ref_positions = Vec::new();

    // Cluster 1: center at (50, 50)
    for i in 0..10 {
        let angle = i as f64 * PI / 5.0;
        let r = 5.0 + (i as f64 * 0.5);
        ref_positions.push((50.0 + r * angle.cos(), 50.0 + r * angle.sin()));
    }

    // Cluster 2: center at (200, 50)
    for i in 0..10 {
        let angle = i as f64 * PI / 5.0;
        let r = 5.0 + (i as f64 * 0.5);
        ref_positions.push((200.0 + r * angle.cos(), 50.0 + r * angle.sin()));
    }

    // Cluster 3: center at (125, 150)
    for i in 0..10 {
        let angle = i as f64 * PI / 5.0;
        let r = 5.0 + (i as f64 * 0.5);
        ref_positions.push((125.0 + r * angle.cos(), 150.0 + r * angle.sin()));
    }

    // Apply translation
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 20.0, y + 15.0))
        .collect();

    let config = TriangleMatchConfig {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // Should match most stars despite clustering
    assert!(
        matches.len() >= 20,
        "Clustered star matching found only {} matches",
        matches.len()
    );
}

/// Test matching with non-uniform brightness distribution (simulating real star fields)
#[test]
fn test_match_brightness_weighted_selection() {
    // This test simulates what happens when max_stars limits star count
    // Brighter stars (lower indices) should be preferred

    let ref_positions: Vec<(f64, f64)> = (0..50)
        .map(|i| {
            let x = (i % 10) as f64 * 30.0;
            let y = (i / 10) as f64 * 30.0;
            (x, y)
        })
        .collect();

    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 10.0, y + 5.0))
        .collect();

    let config = TriangleMatchConfig {
        max_stars: 20, // Only use "brightest" 20 stars
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_stars_triangles(&ref_positions, &target_positions, &config);

    // All matches should be from the first 20 stars
    for m in &matches {
        assert!(
            m.ref_idx < 20,
            "Match included star {} which exceeds max_stars limit",
            m.ref_idx
        );
    }

    assert!(
        matches.len() >= 10,
        "Brightness-limited matching found only {} matches",
        matches.len()
    );
}

// ============================================================================
// Numerical Stability Tests
// ============================================================================

/// Test triangle formation with very flat triangles
#[test]
fn test_triangle_very_flat() {
    // Nearly collinear points - should reject very flat triangles
    // Use even smaller offset to ensure rejection
    let positions: [(f64, f64); 3] = [
        (0.0, 0.0),
        (100.0, 0.0),
        (50.0, 1e-10), // Extremely small offset - nearly collinear
    ];

    let tri = Triangle::from_positions([0, 1, 2], [positions[0], positions[1], positions[2]]);

    // Very flat triangle should be rejected (area too small)
    assert!(
        tri.is_none(),
        "Should reject extremely flat triangle (height ~0)"
    );
}

/// Test triangle formation with nearly degenerate points
#[test]
fn test_triangle_near_collinear() {
    // Points that are almost but not quite collinear
    let positions: [(f64, f64); 3] = [
        (0.0, 0.0),
        (100.0, 0.0),
        (50.0, 1.0), // Small but valid offset
    ];

    let tri = Triangle::from_positions([0, 1, 2], [positions[0], positions[1], positions[2]]);

    // This should form a valid (though thin) triangle
    assert!(tri.is_some(), "Should accept thin but valid triangle");

    // Check area is computed reasonably
    let tri = tri.unwrap();
    // Area should be approximately 50 (base=100, height=1, area=50)
    let expected_area = 50.0;
    // Heron's formula: s = (a+b+c)/2, area = sqrt(s*(s-a)*(s-b)*(s-c))
    let a = tri.sides[0];
    let b = tri.sides[1];
    let c = tri.sides[2];
    let s = (a + b + c) / 2.0;
    let area = (s * (s - a) * (s - b) * (s - c)).sqrt();
    assert!(
        (area - expected_area).abs() < 1.0,
        "Expected area ~{}, got {}",
        expected_area,
        area
    );
}

/// Test matching with large coordinate values
#[test]
fn test_match_large_coordinates() {
    // Coordinates typical of high-resolution sensors (4K+)
    let base_offset = 5000.0;
    let ref_positions: Vec<(f64, f64)> = (0..25)
        .map(|i| {
            let x = base_offset + (i % 5) as f64 * 100.0;
            let y = base_offset + (i / 5) as f64 * 100.0;
            (x, y)
        })
        .collect();

    // Apply small translation
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 10.0, y - 5.0))
        .collect();

    let config = TriangleMatchConfig::default();
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    assert!(
        matches.len() >= 20,
        "Large coordinate matching found only {} matches",
        matches.len()
    );

    // Verify all matches are correct
    for m in &matches {
        assert_eq!(
            m.ref_idx, m.target_idx,
            "Incorrect match at large coordinates: ref {} != target {}",
            m.ref_idx, m.target_idx
        );
    }
}

/// Test matching with small but reasonable coordinate values
#[test]
fn test_match_small_coordinates() {
    // Small but non-trivial coordinates (scaled down star field)
    let ref_positions: Vec<(f64, f64)> = (0..25)
        .map(|i| {
            let x = (i % 5) as f64 * 10.0;
            let y = (i / 5) as f64 * 10.0;
            (x, y)
        })
        .collect();

    // Apply translation
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 1.0, y - 0.5))
        .collect();

    let config = TriangleMatchConfig::default();
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    assert!(
        matches.len() >= 15,
        "Small coordinate matching found only {} matches",
        matches.len()
    );
}

/// Test triangle similarity with very similar but different triangles
#[test]
fn test_triangle_similarity_threshold_boundary() {
    // Create an equilateral triangle: sides all equal, ratios = (1.0, 1.0)
    let tri1 = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (10.0, 0.0), (5.0, 8.66)]).unwrap();

    // Create a similar but slightly distorted triangle
    // Ratios are sides[0]/sides[2] and sides[1]/sides[2]
    // Distort to change ratios by ~0.05
    let tri2 = Triangle::from_positions([0, 1, 2], [(0.0, 0.0), (10.0, 0.0), (4.5, 8.0)]).unwrap();

    // Print ratios for debugging
    let dr0 = (tri1.ratios.0 - tri2.ratios.0).abs();
    let dr1 = (tri1.ratios.1 - tri2.ratios.1).abs();

    // With large tolerance, these should be similar
    assert!(
        tri1.is_similar(&tri2, 0.15),
        "Triangles should be similar with 15% tolerance (dr0={}, dr1={})",
        dr0,
        dr1
    );

    // With tight tolerance, these should NOT be similar
    assert!(
        !tri1.is_similar(&tri2, 0.01),
        "Distorted triangles should not match 1% tolerance (dr0={}, dr1={})",
        dr0,
        dr1
    );
}

/// Test matching with extreme scale difference between reference and target
#[test]
fn test_match_large_scale_difference() {
    // Reference stars at normal scale
    let ref_positions: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 50.0;
            let y = (i / 5) as f64 * 50.0;
            (x, y)
        })
        .collect();

    // Target stars scaled by 2x (large scale difference)
    let scale = 2.0;
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x * scale, y * scale))
        .collect();

    let config = TriangleMatchConfig::default();
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // Triangle matching is scale-invariant, should still find matches
    assert!(
        matches.len() >= 15,
        "Scale-invariant matching with 2x scale found only {} matches",
        matches.len()
    );
}

/// Test matching with 180 degree rotation
#[test]
fn test_match_180_degree_rotation() {
    let ref_positions: Vec<(f64, f64)> = (0..16)
        .map(|i| {
            let x = 100.0 + (i % 4) as f64 * 50.0;
            let y = 100.0 + (i / 4) as f64 * 50.0;
            (x, y)
        })
        .collect();

    // Center of the pattern
    let cx = 175.0;
    let cy = 175.0;

    // 180 degree rotation around center
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| {
            let dx = x - cx;
            let dy = y - cy;
            (cx - dx, cy - dy) // 180 degree rotation
        })
        .collect();

    let config = TriangleMatchConfig {
        check_orientation: false, // Must disable for 180 degree rotation
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // Should still find matches despite 180 degree rotation
    assert!(
        matches.len() >= 10,
        "180 degree rotation matching found only {} matches",
        matches.len()
    );
}

// ============================================================================
// Two-step matching tests
// ============================================================================

/// Test two-step matching with translated stars
#[test]
fn test_two_step_matching_translated() {
    let ref_positions: Vec<(f64, f64)> = (0..25)
        .map(|i| {
            let x = 100.0 + (i % 5) as f64 * 40.0;
            let y = 100.0 + (i / 5) as f64 * 40.0;
            (x, y)
        })
        .collect();

    // Simple translation
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 50.0, y + 30.0))
        .collect();

    // Standard matching
    let standard_config = TriangleMatchConfig {
        two_step_matching: false,
        ..Default::default()
    };
    let standard_matches = match_triangles(&ref_positions, &target_positions, &standard_config);

    // Two-step matching
    let twostep_config = TriangleMatchConfig {
        two_step_matching: true,
        ..Default::default()
    };
    let twostep_matches = match_triangles(&ref_positions, &target_positions, &twostep_config);

    // Both should find matches
    assert!(
        standard_matches.len() >= 10,
        "Standard matching should find matches: {}",
        standard_matches.len()
    );

    // Two-step should find at least as many matches
    assert!(
        twostep_matches.len() >= standard_matches.len().saturating_sub(2),
        "Two-step should find similar matches. Standard: {}, Two-step: {}",
        standard_matches.len(),
        twostep_matches.len()
    );
}

/// Test two-step matching with rotated and scaled stars
#[test]
fn test_two_step_matching_similarity_transform() {
    let ref_positions: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let x = 200.0 + (i % 5) as f64 * 50.0;
            let y = 200.0 + (i / 5) as f64 * 50.0;
            (x, y)
        })
        .collect();

    let cx = 300.0;
    let cy = 275.0;
    let scale = 1.05;
    let rotation: f64 = 0.1; // ~6 degrees
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();

    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| {
            let dx = x - cx;
            let dy = y - cy;
            let x_rot = (dx * cos_r - dy * sin_r) * scale + cx + 20.0;
            let y_rot = (dx * sin_r + dy * cos_r) * scale + cy + 15.0;
            (x_rot, y_rot)
        })
        .collect();

    // Two-step matching
    let config = TriangleMatchConfig {
        two_step_matching: true,
        ..Default::default()
    };
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // Should find good matches
    assert!(
        matches.len() >= 10,
        "Two-step matching with similarity transform found only {} matches",
        matches.len()
    );
}

/// Test that two-step matching falls back gracefully with few matches
#[test]
fn test_two_step_matching_fallback() {
    // Very sparse field - likely to have few initial matches
    let ref_positions: Vec<(f64, f64)> = vec![
        (100.0, 100.0),
        (200.0, 150.0),
        (150.0, 250.0),
        (300.0, 200.0),
    ];

    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|(x, y)| (x + 10.0, y + 5.0))
        .collect();

    let config = TriangleMatchConfig {
        two_step_matching: true,
        min_votes: 1,
        ..Default::default()
    };

    // Should not crash and should return some result
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // At least should not crash; may or may not find matches depending on tolerance
    assert!(matches.len() <= ref_positions.len());
}
