use crate::stacking::registration::triangle::tests::*;

#[test]
fn test_form_triangles_from_neighbors_single_triangle() {
    // 3 points → exactly 1 triangle: [0, 1, 2]
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.5, 0.866),
    ];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);
    assert_eq!(triangles.len(), 1);
    assert_eq!(triangles[0], [0, 1, 2]);
}

#[test]
fn test_form_triangles_from_neighbors_square() {
    // 4 points forming a square → C(4,3) = 4 triangles with k=3 (all neighbors)
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);

    // With 4 points and k=3 (all neighbors), all C(4,3)=4 triangles should be found
    assert_eq!(triangles.len(), 4);

    // All indices should be sorted and valid
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
fn test_form_triangles_from_neighbors_k1_insufficient() {
    // With k=1, each point only has 1 neighbor. Need 2 neighbors to form a triangle.
    // So no triangles should be formed (you need at least 2 neighbors of point i
    // to pair them into a triangle).
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(200.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    // k=1: each point gets 1 neighbor. With only 1 neighbor per point,
    // we need pairs of neighbors, so can't form triangles from just 1 neighbor.
    // Actually the loop needs at least 2 neighbors (ni and n2 must be different).
    // But k=1 means k_nearest returns point itself + 1 neighbor = 2 results,
    // so after filtering self, only 1 neighbor remains. Skip(ni+1) means no n2 → no triangles.
    let triangles = form_triangles_from_neighbors(&tree, 1);
    assert!(
        triangles.is_empty(),
        "k=1 should produce no triangles, got {}",
        triangles.len()
    );
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
fn test_form_triangles_full_k_equals_brute_force() {
    // With k = n-1 (all neighbors), should produce C(n,3) = n*(n-1)*(n-2)/6 triangles
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 1.0),
    ];

    let n = points.len();
    // C(6,3) = 6*5*4/6 = 20
    let brute_force_count = n * (n - 1) * (n - 2) / 6;
    assert_eq!(brute_force_count, 20);

    let tree = KdTree::build(&points).unwrap();
    let triangles = form_triangles_from_neighbors(&tree, n - 1);
    assert_eq!(triangles.len(), brute_force_count);
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
fn test_form_triangles_kdtree_single_triangle() {
    // 3 points forming a 3-4-5 triangle
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(0.0, 4.0),
    ];

    let triangles = form_triangles_kdtree(&positions, 3);

    // Exactly 1 valid triangle from 3 points
    assert_eq!(triangles.len(), 1);
    // Ratios should be (3/5, 4/5) = (0.6, 0.8)
    assert!((triangles[0].ratios.0 - 0.6).abs() < 1e-10);
    assert!((triangles[0].ratios.1 - 0.8).abs() < 1e-10);
}

#[test]
fn test_form_triangles_kdtree_all_collinear() {
    // All collinear points produce no valid triangles
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
fn test_form_triangles_kdtree_ratios_in_valid_range() {
    // 5 points forming a non-degenerate pattern
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let triangles = form_triangles_kdtree(&positions, 4);

    // Should form multiple triangles from 5 points
    assert!(triangles.len() >= 4);

    // All ratios must satisfy 0 < ratio.0 <= ratio.1 <= 1.0
    // (sides are sorted, so ratio.0 = shortest/longest <= ratio.1 = middle/longest <= 1.0)
    for tri in &triangles {
        assert!(
            tri.ratios.0 > 0.0 && tri.ratios.0 <= 1.0,
            "ratio.0 = {} out of (0, 1] range",
            tri.ratios.0
        );
        assert!(
            tri.ratios.1 > 0.0 && tri.ratios.1 <= 1.0,
            "ratio.1 = {} out of (0, 1] range",
            tri.ratios.1
        );
        assert!(
            tri.ratios.0 <= tri.ratios.1 + 1e-10,
            "ratio.0 ({}) > ratio.1 ({})",
            tri.ratios.0,
            tri.ratios.1
        );
    }
}
