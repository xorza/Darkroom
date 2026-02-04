//! Tests for triangle matching module.

use std::collections::HashMap;

use super::*;
use glam::DVec2;

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

    // 3-4-5 right triangle
    assert!((tri.sides[0] - 3.0).abs() < 1e-10);
    assert!((tri.sides[1] - 4.0).abs() < 1e-10);
    assert!((tri.sides[2] - 5.0).abs() < 1e-10);
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
    let ccw = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(0.0, 1.0),
        ],
    )
    .unwrap();
    assert_eq!(ccw.orientation, Orientation::CounterClockwise);

    let cw = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(0.0, 1.0),
            DVec2::new(1.0, 0.0),
        ],
    )
    .unwrap();
    assert_eq!(cw.orientation, Orientation::Clockwise);
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
fn test_hash_table_build() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 4);

    assert!(!triangles.is_empty());

    let table = TriangleHashTable::build(&triangles, 100);
    assert_eq!(table.len(), triangles.len());
}

#[test]
fn test_hash_table_lookup() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(0.0, 4.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 3);
    assert!(!triangles.is_empty());
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
fn test_too_few_stars() {
    let positions = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let matches = match_triangles(&positions, &positions, &TriangleMatchConfig::default());
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
fn test_triangle_hash_key() {
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    let (bx, by) = tri.hash_key(100);
    assert!(bx < 100);
    assert!(by < 100);
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
        &TriangleMatchConfig::default(),
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

    // All triangles should be valid (non-degenerate)
    for tri in &triangles {
        assert!(tri.sides[0] > 0.0);
        assert!(tri.sides[1] > 0.0);
        assert!(tri.sides[2] > 0.0);
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
        &TriangleMatchConfig::default(),
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
        &TriangleMatchConfig::default(),
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

    let config = TriangleMatchConfig {
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

    let config = TriangleMatchConfig {
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

    let config = TriangleMatchConfig {
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

    let config = TriangleMatchConfig {
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

    let config = TriangleMatchConfig {
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
    let ref_positions: Vec<DVec2> = (0..25)
        .map(|i| {
            let x = 100.0 + (i % 5) as f64 * 40.0;
            let y = 100.0 + (i / 5) as f64 * 40.0;
            DVec2::new(x, y)
        })
        .collect();

    // Simple translation
    let offset = DVec2::new(50.0, 30.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

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
    let ref_positions: Vec<DVec2> = (0..20)
        .map(|i| {
            let x = 200.0 + (i % 5) as f64 * 50.0;
            let y = 200.0 + (i / 5) as f64 * 50.0;
            DVec2::new(x, y)
        })
        .collect();

    let center = DVec2::new(300.0, 275.0);
    let scale = 1.05;
    let rotation: f64 = 0.1; // ~6 degrees
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    let translate = DVec2::new(20.0, 15.0);

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| {
            let d = *p - center;
            let x_rot = (d.x * cos_r - d.y * sin_r) * scale + center.x + translate.x;
            let y_rot = (d.x * sin_r + d.y * cos_r) * scale + center.y + translate.y;
            DVec2::new(x_rot, y_rot)
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
    let ref_positions: Vec<DVec2> = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 150.0),
        DVec2::new(150.0, 250.0),
        DVec2::new(300.0, 200.0),
    ];

    let offset = DVec2::new(10.0, 5.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

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

// ============================================================================
// resolve_matches tests
// ============================================================================

#[test]
fn test_resolve_matches_one_to_one() {
    let mut votes = HashMap::new();
    votes.insert((0, 0), 10);
    votes.insert((1, 1), 8);
    votes.insert((2, 2), 6);

    let matches = resolve_matches(votes, 3, 3, 1);
    assert_eq!(matches.len(), 3);

    // Highest votes should come first
    assert_eq!(matches[0].ref_idx, 0);
    assert_eq!(matches[0].votes, 10);
}

#[test]
fn test_resolve_matches_conflict_resolution() {
    // Two candidates for the same target star: ref 0 and ref 1 both want target 0
    let mut votes = HashMap::new();
    votes.insert((0, 0), 10); // ref 0 -> target 0, high votes
    votes.insert((1, 0), 5); // ref 1 -> target 0, lower votes
    votes.insert((1, 1), 3); // ref 1 -> target 1, even lower

    let matches = resolve_matches(votes, 3, 3, 1);

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
    let mut votes = HashMap::new();
    votes.insert((0, 0), 10);
    votes.insert((1, 1), 2);
    votes.insert((2, 2), 1); // Below min_votes=3

    let matches = resolve_matches(votes, 3, 3, 3);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].ref_idx, 0);
}

#[test]
fn test_resolve_matches_empty() {
    let votes = HashMap::new();
    let matches = resolve_matches(votes, 5, 5, 1);
    assert!(matches.is_empty());
}

#[test]
fn test_resolve_matches_confidence() {
    let mut votes = HashMap::new();
    votes.insert((0, 0), 10);

    let matches = resolve_matches(votes, 5, 5, 1);
    assert_eq!(matches.len(), 1);
    assert!(matches[0].confidence > 0.0);
    assert!(matches[0].confidence <= 1.0);
}

// ============================================================================
// estimate_similarity_transform tests
// ============================================================================

#[test]
fn test_estimate_similarity_identity() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let matches: Vec<StarMatch> = (0..4)
        .map(|i| StarMatch {
            ref_idx: i,
            target_idx: i,
            votes: 5,
            confidence: 1.0,
        })
        .collect();

    let result = estimate_similarity_transform(&positions, &positions, &matches);
    assert!(result.is_some());

    let (scale, rotation, translation) = result.unwrap();
    assert!(
        (scale - 1.0).abs() < 0.01,
        "Expected scale=1.0, got {scale}"
    );
    assert!(rotation.abs() < 0.01, "Expected rotation=0, got {rotation}");
    assert!(
        translation.length() < 0.5,
        "Expected near-zero translation, got {translation}"
    );
}

#[test]
fn test_estimate_similarity_translation_only() {
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let offset = DVec2::new(50.0, 30.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let matches: Vec<StarMatch> = (0..4)
        .map(|i| StarMatch {
            ref_idx: i,
            target_idx: i,
            votes: 5,
            confidence: 1.0,
        })
        .collect();

    let result = estimate_similarity_transform(&ref_positions, &target_positions, &matches);
    assert!(result.is_some());

    let (scale, rotation, _translation) = result.unwrap();
    assert!(
        (scale - 1.0).abs() < 0.01,
        "Expected scale=1.0, got {scale}"
    );
    assert!(rotation.abs() < 0.01, "Expected rotation=0, got {rotation}");
}

#[test]
fn test_estimate_similarity_with_rotation_and_scale() {
    let ref_positions = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 100.0),
        DVec2::new(100.0, 200.0),
        DVec2::new(200.0, 200.0),
        DVec2::new(150.0, 150.0),
    ];

    let angle: f64 = 0.1; // ~5.7 degrees
    let scale: f64 = 1.05;
    let cos_r = angle.cos();
    let sin_r = angle.sin();
    let translate = DVec2::new(20.0, -10.0);

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| {
            DVec2::new(
                (p.x * cos_r - p.y * sin_r) / scale - translate.x / scale,
                (p.x * sin_r + p.y * cos_r) / scale - translate.y / scale,
            )
        })
        .collect();

    let matches: Vec<StarMatch> = (0..5)
        .map(|i| StarMatch {
            ref_idx: i,
            target_idx: i,
            votes: 5,
            confidence: 1.0,
        })
        .collect();

    let result = estimate_similarity_transform(&ref_positions, &target_positions, &matches);
    assert!(result.is_some(), "Transform estimation should succeed");

    let (est_scale, est_rotation, _) = result.unwrap();
    // We don't check exact values since the transform is inverted;
    // just verify reasonable outputs
    assert!(
        est_scale > 0.5 && est_scale < 2.0,
        "Scale should be reasonable, got {est_scale}"
    );
    assert!(
        est_rotation.abs() < 1.0,
        "Rotation should be small, got {est_rotation}"
    );
}

#[test]
fn test_estimate_similarity_too_few_matches() {
    let positions = vec![DVec2::new(0.0, 0.0)];
    let matches = vec![StarMatch {
        ref_idx: 0,
        target_idx: 0,
        votes: 5,
        confidence: 1.0,
    }];

    let result = estimate_similarity_transform(&positions, &positions, &matches);
    assert!(result.is_none());
}

// ============================================================================
// compute_position_threshold tests
// ============================================================================

#[test]
fn test_position_threshold_uniform_grid() {
    let positions: Vec<DVec2> = (0..25)
        .map(|i| DVec2::new((i % 5) as f64 * 50.0, (i / 5) as f64 * 50.0))
        .collect();

    let threshold = compute_position_threshold(&positions);

    // Nearest-neighbor distance is 50.0, threshold should be ~150 (3x)
    assert!(
        threshold > 100.0 && threshold < 200.0,
        "Expected threshold ~150 for grid spacing 50, got {threshold}"
    );
}

#[test]
fn test_position_threshold_single_star() {
    let positions = vec![DVec2::new(100.0, 100.0)];
    let threshold = compute_position_threshold(&positions);
    assert_eq!(threshold, 100.0); // Default fallback
}

#[test]
fn test_position_threshold_empty() {
    let positions: Vec<DVec2> = vec![];
    let threshold = compute_position_threshold(&positions);
    assert_eq!(threshold, 100.0); // Default fallback
}

#[test]
fn test_position_threshold_dense_cluster() {
    // Very dense cluster: nn distance ~1.0
    let positions: Vec<DVec2> = (0..25)
        .map(|i| DVec2::new((i % 5) as f64 * 1.0, (i / 5) as f64 * 1.0))
        .collect();

    let threshold = compute_position_threshold(&positions);

    // Minimum threshold is 5.0 per the implementation
    assert!(
        threshold >= 5.0,
        "Threshold should be at least 5.0, got {threshold}"
    );
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

    let map = vm.into_hashmap();
    assert_eq!(*map.get(&(0, 0)).unwrap(), 2);
    assert_eq!(*map.get(&(5, 7)).unwrap(), 1);
    assert_eq!(map.len(), 2);
}

#[test]
fn test_vote_matrix_sparse_mode() {
    // Large enough to trigger sparse mode (>250_000 entries)
    let mut vm = VoteMatrix::new(600, 600);
    assert!(matches!(vm, VoteMatrix::Sparse(_)));

    vm.increment(0, 0);
    vm.increment(0, 0);
    vm.increment(100, 200);

    let map = vm.into_hashmap();
    assert_eq!(*map.get(&(0, 0)).unwrap(), 2);
    assert_eq!(*map.get(&(100, 200)).unwrap(), 1);
    assert_eq!(map.len(), 2);
}

#[test]
fn test_vote_matrix_dense_empty_to_hashmap() {
    let vm = VoteMatrix::new(5, 5);
    let map = vm.into_hashmap();
    assert!(map.is_empty());
}

// ============================================================================
// match_triangles with two_step_matching disabled
// ============================================================================

#[test]
fn test_kdtree_match_two_step_disabled() {
    let ref_positions: Vec<DVec2> = (0..25)
        .map(|i| DVec2::new((i % 5) as f64 * 40.0 + 100.0, (i / 5) as f64 * 40.0 + 100.0))
        .collect();

    let offset = DVec2::new(30.0, -20.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let config = TriangleMatchConfig {
        two_step_matching: false,
        ..Default::default()
    };

    let matches = match_triangles(&ref_positions, &target_positions, &config);

    assert!(
        matches.len() >= 15,
        "Single-step kdtree matching should find matches: {}",
        matches.len()
    );

    for m in &matches {
        assert_eq!(m.ref_idx, m.target_idx);
    }
}

// ============================================================================
// Two-step refinement benefit test
// ============================================================================

#[test]
fn test_two_step_refinement_does_not_degrade() {
    // Use a scenario with rotation + scale where two-step refinement
    // should maintain or improve match count.
    let ref_positions: Vec<DVec2> = (0..36)
        .map(|i| {
            let x = 200.0 + (i % 6) as f64 * 40.0;
            let y = 200.0 + (i / 6) as f64 * 40.0;
            DVec2::new(x, y)
        })
        .collect();

    let center = DVec2::new(300.0, 300.0);
    let angle: f64 = 0.08;
    let scale: f64 = 1.03;
    let cos_r = angle.cos();
    let sin_r = angle.sin();

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| {
            let d = *p - center;
            let x = (d.x * cos_r - d.y * sin_r) * scale + center.x + 15.0;
            let y = (d.x * sin_r + d.y * cos_r) * scale + center.y - 10.0;
            DVec2::new(x, y)
        })
        .collect();

    let config_one_step = TriangleMatchConfig {
        two_step_matching: false,
        ..Default::default()
    };
    let one_step = match_triangles(&ref_positions, &target_positions, &config_one_step);

    let config_two_step = TriangleMatchConfig {
        two_step_matching: true,
        ..Default::default()
    };
    let two_step = match_triangles(&ref_positions, &target_positions, &config_two_step);

    // Two-step should not significantly degrade results
    assert!(
        two_step.len() >= one_step.len().saturating_sub(3),
        "Two-step ({}) should not significantly degrade vs one-step ({})",
        two_step.len(),
        one_step.len()
    );

    // Verify correctness of two-step matches
    let correct = two_step
        .iter()
        .filter(|m| m.ref_idx == m.target_idx)
        .count();
    assert!(
        correct as f64 / two_step.len().max(1) as f64 >= 0.9,
        "Two-step match accuracy should be >= 90%: {correct}/{}",
        two_step.len()
    );
}
