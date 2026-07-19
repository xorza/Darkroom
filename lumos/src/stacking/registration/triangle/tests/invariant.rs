use crate::stacking::registration::triangle::tests::*;

#[test]
fn test_invariant_tree_empty() {
    let tree = build_invariant_tree(&[]);
    assert!(tree.is_none());
}

#[test]
fn test_invariant_tree_build_and_size() {
    // Build from known triangles, verify tree size matches triangle count
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(0.0, 4.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 3);
    assert_eq!(triangles.len(), 1);

    let tree = build_invariant_tree(&triangles).unwrap();
    assert_eq!(tree.len(), 1);
}

#[test]
fn test_invariant_tree_lookup_finds_self() {
    // A triangle's own ratios should always find itself
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(0.0, 4.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 3);
    let tree = build_invariant_tree(&triangles).unwrap();

    // Query with the triangle's own ratios (0.6, 0.8)
    let query = DVec2::new(triangles[0].ratios.0, triangles[0].ratios.1);
    let mut candidates = Vec::new();
    tree.radius_indices_into(query, 0.01, &mut candidates);
    assert!(candidates.contains(&0));
}

#[test]
fn test_invariant_tree_finds_similar_triangles() {
    // Two 3-4-5 triangles at different scales have identical ratios (0.6, 0.8)
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
        [3, 4, 5],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(30.0, 0.0),
            DVec2::new(0.0, 40.0),
        ],
    )
    .unwrap();

    // Both should have identical ratios
    assert!((tri1.ratios.0 - tri2.ratios.0).abs() < 1e-10);

    let tree = build_invariant_tree(&[tri1.clone(), tri2]).unwrap();
    let query = DVec2::new(tri1.ratios.0, tri1.ratios.1);
    let mut candidates = Vec::new();
    tree.radius_indices_into(query, 0.01, &mut candidates);

    assert_eq!(candidates.len(), 2);
}

#[test]
fn test_invariant_search_zero_tolerance() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(0.0, 4.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 3);
    let tree = build_invariant_tree(&triangles).unwrap();

    // Zero tolerance should still find exact match (distance = 0 <= 0)
    let query = DVec2::new(triangles[0].ratios.0, triangles[0].ratios.1);
    let mut candidates = Vec::new();
    tree.radius_indices_into(query, 0.0, &mut candidates);
    assert!(candidates.contains(&0));
}

#[test]
fn test_invariant_search_large_tolerance_finds_all() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];
    let triangles = form_triangles_kdtree(&positions, 4);
    let n_triangles = triangles.len();
    assert!(n_triangles > 0);

    let tree = build_invariant_tree(&triangles).unwrap();

    // Tolerance of 2.0 covers entire ratio space [0,1]x[0,1] → find all
    let query = DVec2::new(triangles[0].ratios.0, triangles[0].ratios.1);
    let mut candidates = Vec::new();
    tree.radius_indices_into(query, 2.0, &mut candidates);
    assert_eq!(candidates.len(), n_triangles);
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

    assert!(
        !candidates.contains(&999),
        "Buffer should be cleared before use"
    );
}
