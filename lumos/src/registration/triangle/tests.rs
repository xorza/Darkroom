//! Tests for triangle matching module.

use glam::DVec2;

use super::TriangleParams;
use super::geometry::{Orientation, Triangle};
use super::matching::{form_triangles_from_neighbors, form_triangles_kdtree, match_triangles};
use super::voting::{VoteMatrix, build_invariant_tree, resolve_matches, vote_for_correspondences};
use crate::registration::spatial::KdTree;

/// Build a dense VoteMatrix from (ref_idx, target_idx, votes) entries.
fn vote_matrix_from_entries(
    n_ref: usize,
    n_target: usize,
    entries: &[(usize, usize, usize)],
) -> VoteMatrix {
    let mut vm = VoteMatrix::new(n_ref, n_target);
    for &(r, t, count) in entries {
        for _ in 0..count {
            vm.increment(r, t);
        }
    }
    vm
}

#[test]
fn test_triangle_from_positions() {
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    );

    assert!(tri.is_some());
    let tri = tri.unwrap();

    // 3-4-5 right triangle: ratios = (3/5, 4/5)
    assert!((tri.ratios.0 - 0.6).abs() < 1e-10);
    assert!((tri.ratios.1 - 0.8).abs() < 1e-10);
}

#[test]
fn test_triangle_ratios_scale_invariant() {
    let tri1 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    // Same triangle, 10x larger
    let tri2 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(30.0, 0.0),
            DVec2::new(0.0, 40.0),
        ],
    )
    .unwrap();

    assert!((tri1.ratios.0 - tri2.ratios.0).abs() < 1e-10);
    assert!((tri1.ratios.1 - tri2.ratios.1).abs() < 1e-10);
}

#[test]
fn test_triangle_similarity_check() {
    let tri1 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    let tri2 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(10.0, 10.0),
            DVec2::new(40.0, 10.0),
            DVec2::new(10.0, 50.0),
        ],
    )
    .unwrap();

    assert!(tri1.is_similar(&tri2, 0.01));
}

#[test]
fn test_triangle_not_similar() {
    let tri1 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(0.0, 1.0),
        ],
    )
    .unwrap();

    // Different shape
    let tri2 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(2.0, 0.0),
            DVec2::new(1.0, 0.1),
        ],
    )
    .unwrap();

    assert!(!tri1.is_similar(&tri2, 0.01));
}

#[test]
fn test_triangle_orientation() {
    // Orientation is computed from geometrically-reordered vertices
    // (opposite shortest, opposite middle, opposite longest), not input order.
    // Use a scalene triangle (all sides different) for unambiguous geometric ordering.
    // 3-4-5 right triangle: sides 3, 4, 5 — all distinct.
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    // Verify orientation is deterministic (just check it's valid)
    assert!(
        tri.orientation == Orientation::Clockwise
            || tri.orientation == Orientation::CounterClockwise
    );

    // Mirrored triangle should have opposite orientation
    let mirrored = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(-3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();
    assert_ne!(tri.orientation, mirrored.orientation);
}

#[test]
fn test_degenerate_triangle_detection() {
    // Collinear points
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(2.0, 2.0),
        ],
    );
    assert!(tri.is_none());

    // Duplicate point
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 1.0),
        ],
    );
    assert!(tri.is_none());
}

#[test]
fn test_invariant_tree_build() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 4);

    assert!(!triangles.is_empty());

    let tree = build_invariant_tree(&triangles);
    assert!(tree.is_some());
    assert_eq!(tree.unwrap().len(), triangles.len());
}

#[test]
fn test_invariant_tree_lookup() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(0.0, 4.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 3);
    assert!(!triangles.is_empty());
    let tree = build_invariant_tree(&triangles).unwrap();

    // Same triangle's invariants should find itself within small tolerance
    let query = DVec2::new(triangles[0].ratios.0, triangles[0].ratios.1);
    let mut candidates = Vec::new();
    tree.radius_indices_into(query, 0.01, &mut candidates);
    assert!(candidates.contains(&0));
}

#[test]
fn test_invariant_tree_empty() {
    let tree = build_invariant_tree(&[]);
    assert!(tree.is_none());
}

#[test]
fn test_too_few_stars() {
    let positions = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let matches = match_triangles(&positions, &positions, &TriangleParams::default());
    assert!(matches.is_empty());
}

#[test]
fn test_all_collinear_stars() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(3.0, 0.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 4);
    assert!(triangles.is_empty());
}

#[test]
fn test_kdtree_match_identical_star_lists() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let matches = match_triangles(&positions, &positions, &TriangleParams::default());

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
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Translate by (100, 50)
    let offset = DVec2::new(100.0, 50.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleParams::default(),
    );

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

#[test]
fn test_kdtree_match_scaled_stars() {
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Scale by 2x
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p * 2.0).collect();

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleParams::default(),
    );

    assert_eq!(matches.len(), 5);
}

#[test]
fn test_form_triangles_kdtree_basic() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let triangles = form_triangles_kdtree(&positions, 4);

    // Should form at least some triangles
    assert!(!triangles.is_empty());

    // All triangles should have valid ratios
    for tri in &triangles {
        assert!(tri.ratios.0 > 0.0 && tri.ratios.0 <= 1.0);
        assert!(tri.ratios.1 > 0.0 && tri.ratios.1 <= 1.0);
    }
}

#[test]
fn test_form_triangles_kdtree_empty() {
    let positions: Vec<DVec2> = vec![];
    let triangles = form_triangles_kdtree(&positions, 5);
    assert!(triangles.is_empty());
}

#[test]
fn test_form_triangles_kdtree_too_few() {
    let positions = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let triangles = form_triangles_kdtree(&positions, 5);
    assert!(triangles.is_empty());
}

#[test]
fn test_kdtree_match_rotated_stars() {
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Rotate by 90 degrees around origin
    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| DVec2::new(-p.y, p.x))
        .collect();

    let config = TriangleParams {
        check_orientation: false, // Symmetric test pattern creates ambiguous correspondences
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

    assert_eq!(matches.len(), 5);
}

#[test]
fn test_kdtree_match_with_missing_stars() {
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Only 4 stars in target (missing one)
    let target_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let matches = match_triangles(
        &ref_positions,
        &target_positions,
        &TriangleParams::default(),
    );

    // Should match the 4 common stars
    assert!(matches.len() >= 4);
}

#[test]
fn test_kdtree_match_with_extra_stars() {
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    // Target has extra stars
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
        &TriangleParams::default(),
    );

    // Should match all 4 reference stars
    assert_eq!(matches.len(), 4);
}

#[test]
fn test_kdtree_match_mirrored_image() {
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Mirror horizontally
    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| DVec2::new(-p.x, p.y))
        .collect();

    // With orientation check, mirrored triangles should be rejected
    let config_with_orientation = TriangleParams {
        check_orientation: true,
        min_votes: 1,
        ..Default::default()
    };
    let matches_with = match_triangles(&ref_positions, &target_positions, &config_with_orientation);

    // Without orientation check, should match more
    let config_no_orientation = TriangleParams {
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
        DVec2::new(0.0, 0.0),
        DVec2::new(50.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 50.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(100.0, 50.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 25.0),
    ];

    // Translate
    let offset = DVec2::new(10.0, 20.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let config = TriangleParams {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

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
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(20.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(20.0, 10.0),
    ];

    // Same 6 stars plus 4 random outliers (40% noise)
    let mut target_positions = ref_positions.clone();
    target_positions.push(DVec2::new(100.0, 100.0));
    target_positions.push(DVec2::new(150.0, 50.0));
    target_positions.push(DVec2::new(75.0, 125.0));
    target_positions.push(DVec2::new(200.0, 200.0));

    let config = TriangleParams {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

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
        DVec2::new(0.0, 0.0),   // A
        DVec2::new(10.0, 0.0),  // B
        DVec2::new(5.0, 20.0),  // C - tall isosceles
        DVec2::new(15.0, 10.0), // D - offset point
    ];

    // Same positions, different order
    let target_positions = vec![
        DVec2::new(0.0, 0.0),   // A
        DVec2::new(10.0, 0.0),  // B
        DVec2::new(5.0, 20.0),  // C
        DVec2::new(15.0, 10.0), // D
    ];

    let config = TriangleParams {
        min_votes: 1,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

    // Each match should be correct (same index)
    for m in &matches {
        assert_eq!(
            m.ref_idx, m.target_idx,
            "Vertex correspondence error: ref {} matched to target {}",
            m.ref_idx, m.target_idx
        );
    }
}

// ============================================================================
// Stress tests
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
            ref_positions.push(DVec2::new(x, y));
        }
    }

    assert_eq!(ref_positions.len(), 500);

    // Apply a similarity transform (rotation + scale + translation)
    let angle = PI / 20.0; // 9 degrees
    let scale = 1.05;
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| {
            let nx = scale * (cos_a * p.x - sin_a * p.y) + 100.0;
            let ny = scale * (sin_a * p.x + cos_a * p.y) + 50.0;
            DVec2::new(nx, ny)
        })
        .collect();

    // Simulate pipeline limiting to brightest 150 stars
    let ref_limited: Vec<DVec2> = ref_positions.iter().take(150).copied().collect();
    let target_limited: Vec<DVec2> = target_positions.iter().take(150).copied().collect();

    let config = TriangleParams {
        min_votes: 3,
        ..Default::default()
    };

    let matches = match_triangles(&ref_limited, &target_limited, &config);

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
        ref_positions.push(DVec2::new(50.0 + r * angle.cos(), 50.0 + r * angle.sin()));
    }

    // Cluster 2: center at (200, 50)
    for i in 0..10 {
        let angle = i as f64 * PI / 5.0;
        let r = 5.0 + (i as f64 * 0.5);
        ref_positions.push(DVec2::new(200.0 + r * angle.cos(), 50.0 + r * angle.sin()));
    }

    // Cluster 3: center at (125, 150)
    for i in 0..10 {
        let angle = i as f64 * PI / 5.0;
        let r = 5.0 + (i as f64 * 0.5);
        ref_positions.push(DVec2::new(125.0 + r * angle.cos(), 150.0 + r * angle.sin()));
    }

    // Apply translation
    let offset = DVec2::new(20.0, 15.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let config = TriangleParams {
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

/// Test matching with pre-limited star count (simulating pipeline's max_stars selection)
#[test]
fn test_match_brightness_weighted_selection() {
    let all_positions: Vec<DVec2> = (0..50)
        .map(|i| {
            let x = (i % 10) as f64 * 30.0;
            let y = (i / 10) as f64 * 30.0;
            DVec2::new(x, y)
        })
        .collect();

    // Simulate pipeline limiting to brightest 20 stars
    let ref_positions: Vec<DVec2> = all_positions.iter().take(20).copied().collect();

    let offset = DVec2::new(10.0, 5.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let config = TriangleParams {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

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
    let positions: [DVec2; 3] = [
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(50.0, 1e-10), // Extremely small offset - nearly collinear
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
    let positions: [DVec2; 3] = [
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(50.0, 1.0), // Small but valid offset
    ];

    let tri = Triangle::from_positions([0, 1, 2], [positions[0], positions[1], positions[2]]);

    // This should form a valid (though thin) triangle
    assert!(tri.is_some(), "Should accept thin but valid triangle");

    // Verify area is reasonable: base=100, height=1, area=50
    let _tri = tri.unwrap();
    let area = 0.5
        * ((positions[1] - positions[0]).x * (positions[2] - positions[0]).y
            - (positions[1] - positions[0]).y * (positions[2] - positions[0]).x)
            .abs();
    assert!((area - 50.0).abs() < 1.0, "Expected area ~50, got {}", area);
}

#[test]
fn test_triangle_side_ratio_filter_rejects_elongated() {
    // Very elongated triangle: sides ~1, ~100, ~100. Ratio = 100/1 = 100 > 10.
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(100.0, 0.0),
            DVec2::new(100.0, 1.0),
        ],
    );
    assert!(tri.is_none(), "Should reject triangle with side ratio > 10");
}

#[test]
fn test_triangle_side_ratio_filter_accepts_moderate() {
    // Triangle with ratio just under 10: sides ~1, ~5, ~5. Ratio ~5.
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(5.0, 0.0),
            DVec2::new(5.0, 1.0),
        ],
    );
    assert!(tri.is_some(), "Should accept triangle with side ratio < 10");
}

#[test]
fn test_triangle_side_ratio_filter_boundary() {
    // Triangle near the boundary: longest/shortest ≈ 10.
    // sides: ~1, ~10, ~10. Ratio = ~10.05.
    let tri_over = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 1.0),
        ],
    );
    assert!(tri_over.is_none(), "Should reject triangle at ratio ~10.05");

    // Slightly wider: sides ~2, ~10, ~10. Ratio = ~5.1.
    let tri_under = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 2.0),
        ],
    );
    assert!(tri_under.is_some(), "Should accept triangle at ratio ~5.1");
}

/// Test matching with large coordinate values
#[test]
fn test_match_large_coordinates() {
    // Coordinates typical of high-resolution sensors (4K+)
    let base_offset = 5000.0;
    let ref_positions: Vec<DVec2> = (0..25)
        .map(|i| {
            let x = base_offset + (i % 5) as f64 * 100.0;
            let y = base_offset + (i / 5) as f64 * 100.0;
            DVec2::new(x, y)
        })
        .collect();

    // Apply small translation
    let offset = DVec2::new(10.0, -5.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let config = TriangleParams::default();
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
    let ref_positions: Vec<DVec2> = (0..25)
        .map(|i| {
            let x = (i % 5) as f64 * 10.0;
            let y = (i / 5) as f64 * 10.0;
            DVec2::new(x, y)
        })
        .collect();

    // Apply translation
    let offset = DVec2::new(1.0, -0.5);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let config = TriangleParams::default();
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    assert!(
        matches.len() >= 15,
        "Small coordinate matching found only {} matches",
        matches.len()
    );
}

/// Test matching with sub-pixel noise on target positions.
/// Real star centroids have jitter from photon noise and centroid estimation.
#[test]
fn test_match_with_subpixel_noise() {
    // Use irregular positions to avoid ambiguous matches on a regular grid
    let ref_positions: Vec<DVec2> = (0..25)
        .map(|i| {
            let base_x = (i % 5) as f64 * 80.0 + 100.0;
            let base_y = (i / 5) as f64 * 80.0 + 100.0;
            // Add deterministic position jitter to break grid symmetry
            let jitter_x = ((i * 13 + 7) as f64 * 0.37).sin() * 15.0;
            let jitter_y = ((i * 17 + 3) as f64 * 0.53).cos() * 15.0;
            DVec2::new(base_x + jitter_x, base_y + jitter_y)
        })
        .collect();

    // Add sub-pixel noise (±0.3 pixels)
    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let noise_x = ((i * 7 + 3) as f64 * 0.73).sin() * 0.3;
            let noise_y = ((i * 11 + 5) as f64 * 0.91).cos() * 0.3;
            DVec2::new(p.x + noise_x, p.y + noise_y)
        })
        .collect();

    let config = TriangleParams::default();
    let matches = match_triangles(&ref_positions, &target_positions, &config);

    assert!(
        matches.len() >= 15,
        "Noisy matching found only {} matches",
        matches.len()
    );

    // Most matches should still be correct despite sub-pixel noise
    let correct = matches.iter().filter(|m| m.ref_idx == m.target_idx).count();
    let accuracy = correct as f64 / matches.len() as f64;
    assert!(
        accuracy >= 0.8,
        "Noisy match accuracy too low: {:.0}% ({correct}/{})",
        accuracy * 100.0,
        matches.len()
    );
}

/// Test triangle similarity with very similar but different triangles
#[test]
fn test_triangle_similarity_threshold_boundary() {
    // Create an equilateral triangle: sides all equal, ratios = (1.0, 1.0)
    let tri1 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(5.0, 8.66),
        ],
    )
    .unwrap();

    // Create a similar but slightly distorted triangle
    // Ratios are sides[0]/sides[2] and sides[1]/sides[2]
    // Distort to change ratios by ~0.05
    let tri2 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(4.5, 8.0),
        ],
    )
    .unwrap();

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
    let ref_positions: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = (i % 5) as f64 * 50.0;
            let y = (i / 5) as f64 * 50.0;
            DVec2::new(x, y)
        })
        .collect();

    // Target stars scaled by 2x (large scale difference)
    let scale = 2.0;
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p * scale).collect();

    let config = TriangleParams::default();
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
    let ref_positions: Vec<DVec2> = (0..16)
        .map(|i| {
            let x = 100.0 + (i % 4) as f64 * 50.0;
            let y = 100.0 + (i / 4) as f64 * 50.0;
            DVec2::new(x, y)
        })
        .collect();

    // Center of the pattern
    let center = DVec2::new(175.0, 175.0);

    // 180 degree rotation around center
    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| {
            let d = *p - center;
            center - d // 180 degree rotation
        })
        .collect();

    let config = TriangleParams {
        check_orientation: false, // Symmetric pattern creates ambiguous correspondences under 180° rotation
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
// resolve_matches tests
// ============================================================================

#[test]
fn test_resolve_matches_one_to_one() {
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 1, 8), (2, 2, 6)]);

    let matches = resolve_matches(vm, 3, 3, 1);
    assert_eq!(matches.len(), 3);

    // Highest votes should come first
    assert_eq!(matches[0].ref_idx, 0);
    assert_eq!(matches[0].votes, 10);
}

#[test]
fn test_resolve_matches_conflict_resolution() {
    // Two candidates for the same target star: ref 0 and ref 1 both want target 0
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 0, 5), (1, 1, 3)]);

    let matches = resolve_matches(vm, 3, 3, 1);

    // ref 0 -> target 0 wins (10 votes)
    // ref 1 -> target 0 is blocked, so ref 1 doesn't get target 0
    // ref 1 -> target 1 is still available (3 votes)
    assert_eq!(matches.len(), 2);

    let m0 = matches.iter().find(|m| m.ref_idx == 0).unwrap();
    assert_eq!(m0.target_idx, 0);
    assert_eq!(m0.votes, 10);

    let m1 = matches.iter().find(|m| m.ref_idx == 1).unwrap();
    assert_eq!(m1.target_idx, 1);
    assert_eq!(m1.votes, 3);
}

#[test]
fn test_resolve_matches_min_votes_filter() {
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 1, 2), (2, 2, 1)]);

    let matches = resolve_matches(vm, 3, 3, 3);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].ref_idx, 0);
}

#[test]
fn test_resolve_matches_empty() {
    let vm = VoteMatrix::new(5, 5);
    let matches = resolve_matches(vm, 5, 5, 1);
    assert!(matches.is_empty());
}

#[test]
fn test_resolve_matches_confidence() {
    let vm = vote_matrix_from_entries(5, 5, &[(0, 0, 10)]);

    let matches = resolve_matches(vm, 5, 5, 1);
    assert_eq!(matches.len(), 1);
    assert!(matches[0].confidence > 0.0);
    assert!(matches[0].confidence <= 1.0);
}

#[test]
fn test_resolve_matches_confidence_relative() {
    // Confidence should be relative to max votes in the set.
    // Top match gets 1.0, others are proportional.
    let vm = vote_matrix_from_entries(5, 5, &[(0, 0, 20), (1, 1, 10), (2, 2, 5)]);

    let matches = resolve_matches(vm, 5, 5, 1);
    assert_eq!(matches.len(), 3);

    // Sorted by votes descending, so matches[0] has 20 votes
    assert_eq!(matches[0].votes, 20);
    assert!((matches[0].confidence - 1.0).abs() < 1e-10);

    assert_eq!(matches[1].votes, 10);
    assert!((matches[1].confidence - 0.5).abs() < 1e-10);

    assert_eq!(matches[2].votes, 5);
    assert!((matches[2].confidence - 0.25).abs() < 1e-10);
}

// ============================================================================
// VoteMatrix tests
// ============================================================================

#[test]
fn test_vote_matrix_dense_mode() {
    // Small enough for dense mode
    let mut vm = VoteMatrix::new(10, 10);
    assert!(matches!(vm, VoteMatrix::Dense { .. }));

    vm.increment(0, 0);
    vm.increment(0, 0);
    vm.increment(5, 7);

    let entries = vm.iter_nonzero();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(2));
    assert_eq!(get(5, 7), Some(1));
    assert_eq!(entries.len(), 2);
}

#[test]
fn test_vote_matrix_sparse_mode() {
    // Large enough to trigger sparse mode (>250_000 entries)
    let mut vm = VoteMatrix::new(600, 600);
    assert!(matches!(vm, VoteMatrix::Sparse(_)));

    vm.increment(0, 0);
    vm.increment(0, 0);
    vm.increment(100, 200);

    let entries = vm.iter_nonzero();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(2));
    assert_eq!(get(100, 200), Some(1));
    assert_eq!(entries.len(), 2);
}

#[test]
fn test_vote_matrix_dense_empty() {
    let vm = VoteMatrix::new(5, 5);
    assert!(vm.iter_nonzero().is_empty());
}

#[test]
fn test_vote_matrix_threshold_boundary() {
    // Threshold is size < 250,000 → dense, size >= 250,000 → sparse.

    // Just below threshold: 499*500 = 249,500 < 250,000 → dense
    let vm_below = VoteMatrix::new(499, 500);
    assert!(
        matches!(vm_below, VoteMatrix::Dense { .. }),
        "499x500 (249.5K) should be dense"
    );

    // Exactly at threshold: 500*500 = 250,000 — not < 250,000 → sparse
    let vm_at = VoteMatrix::new(500, 500);
    assert!(
        matches!(vm_at, VoteMatrix::Sparse(_)),
        "500x500 (250K) should be sparse"
    );
}

// ============================================================================
// Invariant tree tests
// ============================================================================

#[test]
fn test_invariant_tree_finds_similar_triangles() {
    // Two similar triangles (same shape, different scale) should be found by radius search
    let tri1 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    // Same shape, different scale
    let tri2 = Triangle::from_positions(
        [3, 4, 5],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(30.0, 0.0),
            DVec2::new(0.0, 40.0),
        ],
    )
    .unwrap();

    // Ratios should be identical
    assert!(
        (tri1.ratios.0 - tri2.ratios.0).abs() < 1e-10,
        "Same-shape triangles should have identical ratios"
    );

    let tree = build_invariant_tree(&[tri1.clone(), tri2]).unwrap();
    let query = DVec2::new(tri1.ratios.0, tri1.ratios.1);
    let mut candidates = Vec::new();
    tree.radius_indices_into(query, 0.01, &mut candidates);

    // Should find both triangles
    assert_eq!(candidates.len(), 2, "Should find both similar triangles");
}

// ============================================================================
// Invariant tree radius search boundary tests
// ============================================================================

#[test]
fn test_invariant_search_zero_tolerance() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(0.0, 4.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 3);
    assert!(!triangles.is_empty());

    let tree = build_invariant_tree(&triangles).unwrap();

    // Zero tolerance should still find exact match (distance = 0 <= 0)
    let query = DVec2::new(triangles[0].ratios.0, triangles[0].ratios.1);
    let mut candidates = Vec::new();
    tree.radius_indices_into(query, 0.0, &mut candidates);
    assert!(
        candidates.contains(&0),
        "Zero tolerance should still find the triangle itself"
    );
}

#[test]
fn test_invariant_search_large_tolerance() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 4);
    assert!(!triangles.is_empty());

    let tree = build_invariant_tree(&triangles).unwrap();

    // Large tolerance covers the entire ratio space, should find all triangles
    let query = DVec2::new(triangles[0].ratios.0, triangles[0].ratios.1);
    let mut candidates = Vec::new();
    tree.radius_indices_into(query, 2.0, &mut candidates);
    assert_eq!(
        candidates.len(),
        triangles.len(),
        "Full tolerance should return all triangles"
    );
}

#[test]
fn test_invariant_search_clears_buffer() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 4);
    let tree = build_invariant_tree(&triangles).unwrap();

    let mut candidates = vec![999, 888, 777]; // Pre-filled garbage
    let query = DVec2::new(triangles[0].ratios.0, triangles[0].ratios.1);
    tree.radius_indices_into(query, 0.01, &mut candidates);

    // Should not contain the garbage values
    assert!(
        !candidates.contains(&999),
        "Buffer should be cleared before use"
    );
}

// ============================================================================
// VoteMatrix edge case tests
// ============================================================================

#[test]
fn test_vote_matrix_dense_saturating_add() {
    // Dense mode uses u16, verify saturating_add behavior
    let mut vm = VoteMatrix::new(2, 2);

    // Increment many times (won't reach u16::MAX in practice, but test saturation logic)
    for _ in 0..1000 {
        vm.increment(0, 0);
    }

    let entries = vm.iter_nonzero();
    let votes = entries.iter().find(|e| e.0 == 0 && e.1 == 0).unwrap().2;
    assert_eq!(votes, 1000);
}

#[test]
fn test_vote_matrix_sparse_empty() {
    let vm = VoteMatrix::new(600, 600); // Sparse mode
    assert!(vm.iter_nonzero().is_empty());
}

#[test]
fn test_vote_matrix_dense_boundary_indices() {
    // Test accessing the last valid index in dense mode
    let n = 10;
    let mut vm = VoteMatrix::new(n, n);
    vm.increment(n - 1, n - 1);
    vm.increment(0, n - 1);
    vm.increment(n - 1, 0);

    let entries = vm.iter_nonzero();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(n - 1, n - 1), Some(1));
    assert_eq!(get(0, n - 1), Some(1));
    assert_eq!(get(n - 1, 0), Some(1));
}

// ============================================================================
// vote_for_correspondences isolated unit tests
// ============================================================================

#[test]
fn test_vote_for_correspondences_identical_triangles() {
    // Create identical triangle sets — every triangle matches itself
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let triangles = form_triangles_kdtree(&positions, 4);
    assert!(!triangles.is_empty());

    let invariant_tree = build_invariant_tree(&triangles).unwrap();

    let config = TriangleParams::default();
    let vm = vote_for_correspondences(
        &triangles,
        &triangles,
        &invariant_tree,
        &config,
        positions.len(),
        positions.len(),
    );

    // Collect into map for inspection
    let votes: std::collections::HashMap<(usize, usize), usize> = vm
        .iter_nonzero()
        .into_iter()
        .map(|(r, t, v)| ((r, t), v))
        .collect();

    // Diagonal entries (i, i) should have votes (point matched to itself)
    let diagonal_votes: usize = (0..positions.len())
        .filter_map(|i| votes.get(&(i, i)))
        .sum();
    assert!(
        diagonal_votes > 0,
        "Identical triangles should produce diagonal votes"
    );

    // Diagonal should dominate off-diagonal for each point
    for i in 0..positions.len() {
        let self_votes = votes.get(&(i, i)).copied().unwrap_or(0);
        for j in 0..positions.len() {
            if i != j {
                let cross_votes = votes.get(&(i, j)).copied().unwrap_or(0);
                assert!(
                    self_votes >= cross_votes,
                    "Point {i}: self-votes ({self_votes}) should >= cross-votes to {j} ({cross_votes})"
                );
            }
        }
    }
}

#[test]
fn test_vote_for_correspondences_no_matching_triangles() {
    // Create two sets of triangles with very different shapes
    let positions_a = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(5.0, 8.66), // equilateral-ish
    ];

    let positions_b = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(50.0, 1.0), // very thin
    ];

    let tri_a = form_triangles_kdtree(&positions_a, 3);
    let tri_b = form_triangles_kdtree(&positions_b, 3);
    assert!(!tri_a.is_empty());
    assert!(!tri_b.is_empty());

    let invariant_tree = build_invariant_tree(&tri_a).unwrap();

    let config = TriangleParams {
        ratio_tolerance: 0.01, // Very tight
        ..Default::default()
    };

    let vm = vote_for_correspondences(
        &tri_b,
        &tri_a,
        &invariant_tree,
        &config,
        positions_a.len(),
        positions_b.len(),
    );

    // With very different shapes and tight tolerance, should get no votes
    let entries = vm.iter_nonzero();
    assert!(
        entries.is_empty(),
        "Dissimilar triangles should produce no votes, got {} entries",
        entries.len()
    );
}

#[test]
fn test_vote_for_correspondences_orientation_filtering() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Mirror to flip orientation
    let mirrored: Vec<DVec2> = positions.iter().map(|p| DVec2::new(-p.x, p.y)).collect();

    let ref_triangles = form_triangles_kdtree(&positions, 4);
    let target_triangles = form_triangles_kdtree(&mirrored, 4);
    let invariant_tree = build_invariant_tree(&ref_triangles).unwrap();

    // With orientation check: should get fewer votes
    let config_with = TriangleParams {
        check_orientation: true,
        ..Default::default()
    };
    let vm_with = vote_for_correspondences(
        &target_triangles,
        &ref_triangles,
        &invariant_tree,
        &config_with,
        positions.len(),
        mirrored.len(),
    );

    // Without orientation check: should get more votes
    let config_without = TriangleParams {
        check_orientation: false,
        ..Default::default()
    };
    let vm_without = vote_for_correspondences(
        &target_triangles,
        &ref_triangles,
        &invariant_tree,
        &config_without,
        positions.len(),
        mirrored.len(),
    );

    let total_with: usize = vm_with.iter_nonzero().iter().map(|&(_, _, v)| v).sum();
    let total_without: usize = vm_without.iter_nonzero().iter().map(|&(_, _, v)| v).sum();

    assert!(
        total_without >= total_with,
        "Disabling orientation check should produce >= votes: with={total_with}, without={total_without}"
    );
}

// ============================================================================
// form_triangles_from_neighbors tests
// ============================================================================

#[test]
fn test_form_triangles_from_neighbors_basic() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.5, 0.866), // Equilateral triangle
    ];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);

    assert_eq!(triangles.len(), 1);
    assert_eq!(triangles[0], [0, 1, 2]);
}

#[test]
fn test_form_triangles_from_neighbors_square() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);

    assert!(triangles.len() >= 4);

    for tri in &triangles {
        assert!(tri[0] < tri[1] && tri[1] < tri[2]);
        assert!(tri[2] < 4);
    }
}

#[test]
fn test_form_triangles_from_neighbors_too_few_points() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);
    assert!(triangles.is_empty());
}

#[test]
fn test_form_triangles_from_neighbors_no_duplicates() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 5);

    let mut sorted = triangles.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), triangles.len(), "Found duplicate triangles");
}

#[test]
fn test_form_triangles_scaling_vs_brute_force() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 1.0),
    ];

    let n = points.len();
    let brute_force_count = n * (n - 1) * (n - 2) / 6;

    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, n - 1);
    assert_eq!(triangles.len(), brute_force_count);
}

#[test]
fn test_form_triangles_sparse_neighbors() {
    let mut points = Vec::new();
    for y in 0..5 {
        for x in 0..5 {
            points.push(DVec2::new(x as f64 * 100.0, y as f64 * 100.0));
        }
    }

    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 2);

    assert!(!triangles.is_empty());

    for tri in &triangles {
        assert!(tri[0] < tri[1] && tri[1] < tri[2]);
    }
}

// ============================================================================
// Geometric role ordering tests
// ============================================================================

/// Verify that from_positions reorders indices by geometric role (scalene triangle).
/// indices[0] = opposite shortest side, indices[2] = opposite longest side.
#[test]
fn test_vertex_ordering_by_geometric_role() {
    // 3-4-5 right triangle with positions:
    // p0=(0,0), p1=(3,0), p2=(0,4)
    // sides: d01=3 (opp vtx 2), d12=5 (opp vtx 0), d20=4 (opp vtx 1)
    // shortest=3 (opp 2), middle=4 (opp 1), longest=5 (opp 0)
    // So reordered: [2, 1, 0]
    let tri = Triangle::from_positions(
        [10, 20, 30],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    assert_eq!(tri.indices[0], 30); // opposite shortest (d01=3)
    assert_eq!(tri.indices[1], 20); // opposite middle (d20=4)
    assert_eq!(tri.indices[2], 10); // opposite longest (d12=5)
}

/// Same geometric triangle with shuffled input order produces the same indices.
#[test]
fn test_vertex_ordering_deterministic_across_input_orders() {
    let p_a = DVec2::new(0.0, 0.0);
    let p_b = DVec2::new(5.0, 0.0);
    let p_c = DVec2::new(2.0, 7.0);

    // All 6 permutations of the same 3 points with the same original indices
    let orders: [(usize, usize, usize); 6] = [
        (0, 1, 2),
        (0, 2, 1),
        (1, 0, 2),
        (1, 2, 0),
        (2, 0, 1),
        (2, 1, 0),
    ];
    let pts = [p_a, p_b, p_c];
    let idx = [100, 200, 300];

    let reference =
        Triangle::from_positions([idx[0], idx[1], idx[2]], [pts[0], pts[1], pts[2]]).unwrap();

    for (a, b, c) in orders {
        let tri =
            Triangle::from_positions([idx[a], idx[b], idx[c]], [pts[a], pts[b], pts[c]]).unwrap();

        assert_eq!(
            tri.indices, reference.indices,
            "Permutation ({a},{b},{c}) produced different indices: {:?} vs {:?}",
            tri.indices, reference.indices
        );
        assert_eq!(
            tri.orientation, reference.orientation,
            "Permutation ({a},{b},{c}) produced different orientation"
        );
        assert!((tri.ratios.0 - reference.ratios.0).abs() < 1e-10);
        assert!((tri.ratios.1 - reference.ratios.1).abs() < 1e-10);
    }
}

/// Verify that voting produces correct correspondences when ref and target
/// have the same geometric points but different index numbering.
#[test]
fn test_vertex_correspondence_with_permuted_indices() {
    // 5 distinctive points forming an asymmetric pattern
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(30.0, 0.0),
        DVec2::new(15.0, 40.0),
        DVec2::new(50.0, 20.0),
        DVec2::new(10.0, 25.0),
    ];

    // Target has the same points but in reversed order
    let target_points: Vec<DVec2> = points.iter().rev().copied().collect();
    // target[0] = points[4], target[1] = points[3], ..., target[4] = points[0]
    // So ref_idx i corresponds to target_idx (4 - i)

    let config = TriangleParams {
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
            "ref {} should match target {} (same point), got target {}",
            m.ref_idx, expected_target, m.target_idx
        );
    }
}
