use super::*;

#[test]
fn form_triangles_from_neighbors_single_triangle() {
    // 3 points → exactly 1 triangle: [0, 1, 2]
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.5, 0.866),
    ];
    let tree = KdTree::build(points.clone()).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);
    assert_eq!(triangles.len(), 1);
    assert_eq!(triangles[0], [0, 1, 2]);
}

#[test]
fn form_triangles_from_neighbors_square() {
    // 4 points forming a square → C(4,3) = 4 triangles with k=3 (all neighbors)
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ];
    let tree = KdTree::build(points.clone()).unwrap();

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
fn form_triangles_from_neighbors_too_few_points() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let tree = KdTree::build(points.clone()).unwrap();
    let triangles = form_triangles_from_neighbors(&tree, 3);
    assert!(triangles.is_empty());
}

/// With `k` = 1 each point has one neighbour, and a triangle needs two: none form.
#[test]
fn form_triangles_from_neighbors_k1_insufficient() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(200.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];
    let triangles = form_triangles_from_neighbors(&tree(&points), 1);
    assert!(triangles.is_empty(), "{triangles:?}");
}

/// With every point a neighbour of every other, the triangles are all `C(n, 3)` of them, each once:
/// `C(6, 3)` = 20 distinct sorted index triples.
#[test]
fn form_triangles_full_k_equals_brute_force() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 1.0),
    ];

    let n = points.len();
    let brute_force_count = 20;

    let triangles = form_triangles_from_neighbors(&tree(&points), n - 1);
    assert_eq!(triangles.len(), brute_force_count);
    let mut distinct = triangles.clone();
    distinct.dedup();
    assert_eq!(distinct, triangles);
    assert!(
        triangles
            .iter()
            .all(|t| t[0] < t[1] && t[1] < t[2] && t[2] < n)
    );
}

#[test]
fn form_triangles_kdtree_empty() {
    let positions: Vec<DVec2> = vec![];
    let triangles = triangles_of(&positions, 5);
    assert!(triangles.is_empty());
}

#[test]
fn form_triangles_kdtree_too_few() {
    let positions = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let triangles = triangles_of(&positions, 5);
    assert!(triangles.is_empty());
}

#[test]
fn form_triangles_kdtree_single_triangle() {
    let triangles = triangles_of(&THREE_FOUR_FIVE, 3);
    assert_eq!(triangles.len(), 1);
    assert_eq!(triangles[0].ratios, (0.6, 0.8));
}

#[test]
fn form_triangles_kdtree_all_collinear() {
    // All collinear points produce no valid triangles
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(3.0, 0.0),
    ];
    let triangles = triangles_of(&positions, 4);
    assert!(triangles.is_empty());
}

#[test]
fn form_triangles_kdtree_ratios_in_valid_range() {
    // 5 points forming a non-degenerate pattern
    let triangles = triangles_of(&SQUARE_AND_CENTRE, 4);

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
