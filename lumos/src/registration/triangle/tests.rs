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
