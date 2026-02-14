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

// ============================================================================
// Triangle::from_positions tests
// ============================================================================

#[test]
fn test_triangle_from_positions_3_4_5() {
    // 3-4-5 right triangle:
    // p0=(0,0), p1=(3,0), p2=(0,4)
    // d01 = 3, d12 = sqrt(9+16) = 5, d20 = 4
    // Sorted sides: [3, 4, 5]
    // ratios = (3/5, 4/5) = (0.6, 0.8)
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    assert!((tri.ratios.0 - 0.6).abs() < 1e-10);
    assert!((tri.ratios.1 - 0.8).abs() < 1e-10);
}

#[test]
fn test_triangle_equilateral_ratios() {
    // Equilateral triangle: all sides equal = 10.0
    // ratios = (10/10, 10/10) = (1.0, 1.0)
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            // height = 10 * sqrt(3)/2 = 8.6602540378...
            DVec2::new(5.0, 8.660254037844386),
        ],
    )
    .unwrap();

    assert!(
        (tri.ratios.0 - 1.0).abs() < 1e-10,
        "Expected ratio.0 = 1.0, got {}",
        tri.ratios.0
    );
    assert!(
        (tri.ratios.1 - 1.0).abs() < 1e-10,
        "Expected ratio.1 = 1.0, got {}",
        tri.ratios.1
    );
}

#[test]
fn test_triangle_ratios_scale_invariant() {
    // 3-4-5 triangle at scale 1 and scale 10 should have identical ratios = (0.6, 0.8)
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
            DVec2::new(0.0, 0.0),
            DVec2::new(30.0, 0.0),
            DVec2::new(0.0, 40.0),
        ],
    )
    .unwrap();

    assert!((tri1.ratios.0 - tri2.ratios.0).abs() < 1e-10);
    assert!((tri1.ratios.1 - tri2.ratios.1).abs() < 1e-10);
    // Both should be exactly the 3-4-5 ratios
    assert!((tri1.ratios.0 - 0.6).abs() < 1e-10);
    assert!((tri1.ratios.1 - 0.8).abs() < 1e-10);
}

#[test]
fn test_triangle_orientation_exact() {
    // 3-4-5 right triangle: p0=(0,0), p1=(3,0), p2=(0,4)
    // Sorted sides: d01=3 (opp vtx 2), d20=4 (opp vtx 1), d12=5 (opp vtx 0)
    // Reordered vertices: [2, 1, 0] → positions [p2, p1, p0] = [(0,4), (3,0), (0,0)]
    // Cross product: rp0=(0,4), rp1=(3,0), rp2=(0,0)
    // rv01 = (3,0)-(0,4) = (3,-4), rv02 = (0,0)-(0,4) = (0,-4)
    // cross = 3*(-4) - (-4)*0 = -12 < 0 → Clockwise
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    assert_eq!(tri.orientation, Orientation::Clockwise);

    // Mirror x → p0=(0,0), p1=(-3,0), p2=(0,4)
    // Sorted sides are the same lengths: d01=3, d12=5, d20=4
    // Reordered vertices: [2, 1, 0] → positions [p2, p1, p0] = [(0,4), (-3,0), (0,0)]
    // rv01 = (-3,0)-(0,4) = (-3,-4), rv02 = (0,0)-(0,4) = (0,-4)
    // cross = (-3)*(-4) - (-4)*0 = 12 > 0 → CounterClockwise
    let mirrored = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(-3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    assert_eq!(mirrored.orientation, Orientation::CounterClockwise);
}

#[test]
fn test_degenerate_triangle_collinear() {
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(2.0, 2.0),
        ],
    );
    assert!(tri.is_none());
}

#[test]
fn test_degenerate_triangle_duplicate_point() {
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
fn test_triangle_very_flat_rejected() {
    // Nearly collinear: height = 1e-10 on base = 100
    // area = 0.5 * 100 * 1e-10 = 5e-9, area^2 = 2.5e-17 < MIN_TRIANGLE_AREA_SQ (1e-6)
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(100.0, 0.0),
            DVec2::new(50.0, 1e-10),
        ],
    );
    assert!(tri.is_none());
}

#[test]
fn test_triangle_near_collinear_accepted() {
    // Thin triangle with height=1 on base=100
    // Sides: d01=100, d02=sqrt(2500+1)~50.01, d12=sqrt(2500+1)~50.01
    // Sorted: [50.01, 50.01, 100] → ratio ≈ (0.5001, 0.5001)
    // area = 0.5 * 100 * 1 = 50, area^2 = 2500 > 1e-6 → accepted
    // side ratio: 100/50.01 ≈ 2.0 < 10 → accepted
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(100.0, 0.0),
            DVec2::new(50.0, 1.0),
        ],
    );
    assert!(tri.is_some());

    let tri = tri.unwrap();
    // d01 = 100 (longest), d02 = sqrt(50^2 + 1) ≈ 50.00999, d12 = sqrt(50^2 + 1) ≈ 50.00999
    // ratios ≈ (50.01/100, 50.01/100) ≈ (0.5001, 0.5001)
    assert!((tri.ratios.0 - 0.5001).abs() < 0.001);
    assert!((tri.ratios.1 - 0.5001).abs() < 0.001);
}

#[test]
fn test_triangle_side_ratio_filter_rejects_elongated() {
    // Very elongated: p0=(0,0), p1=(100,0), p2=(100,1)
    // d01=100, d12=1, d20=sqrt(10001)≈100.005
    // longest/shortest = 100.005/1 ≈ 100 >> 10 → rejected
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(100.0, 0.0),
            DVec2::new(100.0, 1.0),
        ],
    );
    assert!(tri.is_none());
}

#[test]
fn test_triangle_side_ratio_filter_accepts_moderate() {
    // Moderate: p0=(0,0), p1=(5,0), p2=(5,1)
    // d01=5, d12=1, d20=sqrt(26)≈5.099
    // longest/shortest = 5.099/1 ≈ 5.1 < 10 → accepted
    let tri = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(5.0, 0.0),
            DVec2::new(5.0, 1.0),
        ],
    );
    assert!(tri.is_some());
}

#[test]
fn test_triangle_side_ratio_filter_boundary() {
    // At boundary: p0=(0,0), p1=(10,0), p2=(10,1)
    // d01=10, d12=1, d20=sqrt(101)≈10.05
    // longest/shortest = 10.05/1 ≈ 10.05 > 10 → rejected
    let tri_over = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 1.0),
        ],
    );
    assert!(tri_over.is_none());

    // Wider: p0=(0,0), p1=(10,0), p2=(10,2)
    // d01=10, d12=2, d20=sqrt(104)≈10.198
    // longest/shortest = 10.198/2 ≈ 5.1 < 10 → accepted
    let tri_under = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 2.0),
        ],
    );
    assert!(tri_under.is_some());
}

// ============================================================================
// Triangle::is_similar tests
// ============================================================================

#[test]
fn test_is_similar_identical_triangles() {
    // 3-4-5 at two scales: ratios both (0.6, 0.8) → difference = (0, 0) < any tolerance
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
    // Even with the tightest tolerance above zero
    assert!(tri1.is_similar(&tri2, 1e-9));
}

#[test]
fn test_is_similar_different_triangles() {
    // 1-1-sqrt(2) isoceles right triangle:
    // d01=1, d12=1, d20=sqrt(2)
    // ratios = (1/sqrt(2), 1/sqrt(2)) ≈ (0.7071, 0.7071)
    let tri1 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(0.0, 1.0),
        ],
    )
    .unwrap();

    // Very thin triangle (rejected elongated ones filtered out, so use something different)
    // 2-1-sqrt(5) triangle:
    // p0=(0,0), p1=(2,0), p2=(1,0.1)
    // d01=2, d12=sqrt(1+0.01)≈1.005, d20=sqrt(1+0.01)≈1.005
    // ratios ≈ (1.005/2, 1.005/2) ≈ (0.5025, 0.5025)
    let tri2 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(2.0, 0.0),
            DVec2::new(1.0, 0.1),
        ],
    )
    .unwrap();

    // Ratio difference ≈ |0.7071 - 0.5025| ≈ 0.205 → not similar at 0.01 tolerance
    assert!(!tri1.is_similar(&tri2, 0.01));
    // But should match at 0.3 tolerance
    assert!(tri1.is_similar(&tri2, 0.3));
}

#[test]
fn test_is_similar_with_exact_tolerance_boundary() {
    // 3-4-5: ratios = (0.6, 0.8)
    let tri1 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    // Equilateral: ratios = (1.0, 1.0)
    let tri2 = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(5.0, 8.660254037844386),
        ],
    )
    .unwrap();

    // dr0 = |0.6 - 1.0| = 0.4, dr1 = |0.8 - 1.0| = 0.2
    // is_similar requires BOTH dr0 < tol AND dr1 < tol
    // At tol=0.3, dr0=0.4 >= 0.3 → not similar
    assert!(!tri1.is_similar(&tri2, 0.3));
    // At tol=0.5, both dr0=0.4 < 0.5 and dr1=0.2 < 0.5 → similar
    assert!(tri1.is_similar(&tri2, 0.5));
}

// ============================================================================
// Vertex ordering tests
// ============================================================================

#[test]
fn test_vertex_ordering_by_geometric_role() {
    // 3-4-5 right triangle with arbitrary indices [10, 20, 30]:
    // p0=(0,0), p1=(3,0), p2=(0,4)
    // d01=3 (opp vtx 2=idx30), d12=5 (opp vtx 0=idx10), d20=4 (opp vtx 1=idx20)
    // Sorted by side length: shortest=3(opp idx30), middle=4(opp idx20), longest=5(opp idx10)
    // Reordered indices: [30, 20, 10]
    let tri = Triangle::from_positions(
        [10, 20, 30],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(0.0, 4.0),
        ],
    )
    .unwrap();

    assert_eq!(tri.indices[0], 30); // opposite shortest side (d01=3)
    assert_eq!(tri.indices[1], 20); // opposite middle side (d20=4)
    assert_eq!(tri.indices[2], 10); // opposite longest side (d12=5)
}

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

// ============================================================================
// form_triangles_from_neighbors tests
// ============================================================================

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

// ============================================================================
// form_triangles_kdtree tests
// ============================================================================

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

// ============================================================================
// Invariant tree tests
// ============================================================================

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

// ============================================================================
// VoteMatrix tests
// ============================================================================

#[test]
fn test_vote_matrix_dense_mode() {
    // 10*10 = 100 < 250,000 → dense
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
    // 600*600 = 360,000 >= 250,000 → sparse
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
fn test_vote_matrix_empty() {
    let vm_dense = VoteMatrix::new(5, 5);
    assert!(vm_dense.iter_nonzero().is_empty());

    let vm_sparse = VoteMatrix::new(600, 600);
    assert!(vm_sparse.iter_nonzero().is_empty());
}

#[test]
fn test_vote_matrix_threshold_boundary() {
    // size < 250,000 → dense, size >= 250,000 → sparse

    // 499*500 = 249,500 < 250,000 → dense
    let vm_below = VoteMatrix::new(499, 500);
    assert!(matches!(vm_below, VoteMatrix::Dense { .. }));

    // 500*500 = 250,000, not < 250,000 → sparse
    let vm_at = VoteMatrix::new(500, 500);
    assert!(matches!(vm_at, VoteMatrix::Sparse(_)));
}

#[test]
fn test_vote_matrix_dense_index_mapping() {
    // Verify that dense mode correctly maps (ref_idx, target_idx) → flat index
    // Formula: flat_idx = ref_idx * n_target + target_idx
    let n_ref = 3;
    let n_target = 4;
    let mut vm = VoteMatrix::new(n_ref, n_target);

    // Set specific cells with different vote counts to verify index mapping
    // (0,0) → idx 0, (0,3) → idx 3, (1,2) → idx 6, (2,0) → idx 8, (2,3) → idx 11
    vm.increment(0, 0); // 1 vote at (0,0)
    vm.increment(0, 3);
    vm.increment(0, 3); // 2 votes at (0,3)
    vm.increment(1, 2);
    vm.increment(1, 2);
    vm.increment(1, 2); // 3 votes at (1,2)
    vm.increment(2, 0); // 1 vote at (2,0)
    vm.increment(2, 3);
    vm.increment(2, 3);
    vm.increment(2, 3);
    vm.increment(2, 3); // 4 votes at (2,3)

    let entries = vm.iter_nonzero();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);

    assert_eq!(get(0, 0), Some(1));
    assert_eq!(get(0, 3), Some(2));
    assert_eq!(get(1, 2), Some(3));
    assert_eq!(get(2, 0), Some(1));
    assert_eq!(get(2, 3), Some(4));
    assert_eq!(entries.len(), 5);
}

#[test]
fn test_vote_matrix_dense_boundary_indices() {
    // Test accessing corners: (0,0), (0,n-1), (n-1,0), (n-1,n-1)
    let n = 10;
    let mut vm = VoteMatrix::new(n, n);
    vm.increment(0, 0);
    vm.increment(0, n - 1);
    vm.increment(n - 1, 0);
    vm.increment(n - 1, n - 1);

    let entries = vm.iter_nonzero();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(1));
    assert_eq!(get(0, n - 1), Some(1));
    assert_eq!(get(n - 1, 0), Some(1));
    assert_eq!(get(n - 1, n - 1), Some(1));
    assert_eq!(entries.len(), 4);
}

#[test]
fn test_vote_matrix_dense_saturating_add() {
    // Dense mode uses u16. Verify exact count for reasonable values.
    let mut vm = VoteMatrix::new(2, 2);
    for _ in 0..1000 {
        vm.increment(0, 0);
    }
    let entries = vm.iter_nonzero();
    let votes = entries.iter().find(|e| e.0 == 0 && e.1 == 0).unwrap().2;
    assert_eq!(votes, 1000);
}

// ============================================================================
// resolve_matches tests
// ============================================================================

#[test]
fn test_resolve_matches_one_to_one() {
    // 3 non-conflicting matches sorted by descending votes
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 1, 8), (2, 2, 6)]);

    let matches = resolve_matches(vm, 3, 3, 1);
    assert_eq!(matches.len(), 3);

    // Sorted by votes descending
    assert_eq!(matches[0].ref_idx, 0);
    assert_eq!(matches[0].target_idx, 0);
    assert_eq!(matches[0].votes, 10);

    assert_eq!(matches[1].ref_idx, 1);
    assert_eq!(matches[1].target_idx, 1);
    assert_eq!(matches[1].votes, 8);

    assert_eq!(matches[2].ref_idx, 2);
    assert_eq!(matches[2].target_idx, 2);
    assert_eq!(matches[2].votes, 6);
}

#[test]
fn test_resolve_matches_target_conflict() {
    // Two ref points compete for the same target:
    // ref 0 → target 0 (10 votes), ref 1 → target 0 (5 votes), ref 1 → target 1 (3 votes)
    // Greedy: ref 0 wins target 0, ref 1 falls back to target 1
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 0, 5), (1, 1, 3)]);

    let matches = resolve_matches(vm, 3, 3, 1);
    assert_eq!(matches.len(), 2);

    let m0 = matches.iter().find(|m| m.ref_idx == 0).unwrap();
    assert_eq!(m0.target_idx, 0);
    assert_eq!(m0.votes, 10);

    let m1 = matches.iter().find(|m| m.ref_idx == 1).unwrap();
    assert_eq!(m1.target_idx, 1);
    assert_eq!(m1.votes, 3);
}

#[test]
fn test_resolve_matches_ref_conflict() {
    // Two target points compete for the same ref:
    // ref 0 → target 0 (10 votes), ref 0 → target 1 (5 votes), ref 1 → target 1 (3 votes)
    // Greedy: ref 0 gets target 0 (highest), ref 0 → target 1 blocked (ref 0 used), ref 1 gets target 1
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (0, 1, 5), (1, 1, 3)]);

    let matches = resolve_matches(vm, 3, 3, 1);
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
    // Only ref 0 → target 0 (10 votes) survives min_votes = 3
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 1, 2), (2, 2, 1)]);

    let matches = resolve_matches(vm, 3, 3, 3);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].ref_idx, 0);
    assert_eq!(matches[0].target_idx, 0);
    assert_eq!(matches[0].votes, 10);
}

#[test]
fn test_resolve_matches_empty() {
    let vm = VoteMatrix::new(5, 5);
    let matches = resolve_matches(vm, 5, 5, 1);
    assert!(matches.is_empty());
}

#[test]
fn test_resolve_matches_confidence_relative() {
    // Confidence = votes / max_votes in resolved set
    // Three matches: 20, 10, 5 votes → confidence = 1.0, 0.5, 0.25
    let vm = vote_matrix_from_entries(5, 5, &[(0, 0, 20), (1, 1, 10), (2, 2, 5)]);

    let matches = resolve_matches(vm, 5, 5, 1);
    assert_eq!(matches.len(), 3);

    // matches[0]: 20 votes → 20/20 = 1.0
    assert_eq!(matches[0].votes, 20);
    assert!((matches[0].confidence - 1.0).abs() < 1e-10);

    // matches[1]: 10 votes → 10/20 = 0.5
    assert_eq!(matches[1].votes, 10);
    assert!((matches[1].confidence - 0.5).abs() < 1e-10);

    // matches[2]: 5 votes → 5/20 = 0.25
    assert_eq!(matches[2].votes, 5);
    assert!((matches[2].confidence - 0.25).abs() < 1e-10);
}

#[test]
fn test_resolve_matches_single_entry_confidence_is_1() {
    // Single match: confidence = votes/max_votes = 10/10 = 1.0
    let vm = vote_matrix_from_entries(5, 5, &[(0, 0, 10)]);

    let matches = resolve_matches(vm, 5, 5, 1);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].votes, 10);
    assert!((matches[0].confidence - 1.0).abs() < 1e-10);
}

// ============================================================================
// vote_for_correspondences tests
// ============================================================================

#[test]
fn test_vote_for_correspondences_identical_triangles() {
    // Identical point sets → every triangle matches itself → diagonal dominates
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

    let votes: std::collections::HashMap<(usize, usize), usize> = vm
        .iter_nonzero()
        .into_iter()
        .map(|(r, t, v)| ((r, t), v))
        .collect();

    // Diagonal should dominate: self-votes >= any cross-vote for each point
    for i in 0..positions.len() {
        let self_votes = votes.get(&(i, i)).copied().unwrap_or(0);
        assert!(self_votes > 0, "Point {i} should have self-votes");
        for j in 0..positions.len() {
            if i != j {
                let cross_votes = votes.get(&(i, j)).copied().unwrap_or(0);
                assert!(
                    self_votes >= cross_votes,
                    "Point {i}: self-votes ({self_votes}) < cross-votes to {j} ({cross_votes})"
                );
            }
        }
    }
}

#[test]
fn test_vote_for_correspondences_no_matching_triangles() {
    // Equilateral-ish triangle vs very thin triangle → no matches at tight tolerance
    let positions_a = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(5.0, 8.66), // equilateral, ratios ≈ (1.0, 1.0)
    ];

    let positions_b = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(50.0, 1.0), // very thin, ratios ≈ (0.5, 0.5)
    ];

    let tri_a = form_triangles_kdtree(&positions_a, 3);
    let tri_b = form_triangles_kdtree(&positions_b, 3);
    assert!(!tri_a.is_empty());
    assert!(!tri_b.is_empty());

    let invariant_tree = build_invariant_tree(&tri_a).unwrap();

    let config = TriangleParams {
        ratio_tolerance: 0.01,
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

    assert!(vm.iter_nonzero().is_empty());
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

    // Mirror x to flip all triangle orientations
    let mirrored: Vec<DVec2> = positions.iter().map(|p| DVec2::new(-p.x, p.y)).collect();

    let ref_triangles = form_triangles_kdtree(&positions, 4);
    let target_triangles = form_triangles_kdtree(&mirrored, 4);
    let invariant_tree = build_invariant_tree(&ref_triangles).unwrap();

    // With orientation check: mirrored triangles rejected → fewer/no votes
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

    // Without orientation check: all matching triangles accepted → more votes
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

    // With mirroring, orientation check should block matches
    assert!(
        total_without > total_with,
        "Orientation filtering should reduce votes: with={total_with}, without={total_without}"
    );
}

// ============================================================================
// match_triangles integration tests
// ============================================================================

#[test]
fn test_match_triangles_too_few_points() {
    let two = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let three = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
    ];

    // Both sides need >= 3 points
    assert!(match_triangles(&two, &three, &TriangleParams::default()).is_empty());
    assert!(match_triangles(&three, &two, &TriangleParams::default()).is_empty());
    assert!(match_triangles(&two, &two, &TriangleParams::default()).is_empty());
}

#[test]
fn test_match_triangles_empty_inputs() {
    let empty: Vec<DVec2> = vec![];
    let three = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
    ];
    assert!(match_triangles(&empty, &three, &TriangleParams::default()).is_empty());
    assert!(match_triangles(&three, &empty, &TriangleParams::default()).is_empty());
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

    let matches = match_triangles(&positions, &positions, &TriangleParams::default());

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
        &TriangleParams::default(),
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
        &TriangleParams::default(),
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

    let config = TriangleParams {
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
        &TriangleParams::default(),
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
        &TriangleParams::default(),
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

    let config_with = TriangleParams {
        check_orientation: true,
        min_votes: 1,
        ..Default::default()
    };
    let matches_with = match_triangles(&ref_positions, &target_positions, &config_with);

    let config_without = TriangleParams {
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

    let config = TriangleParams {
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

    let tight = TriangleParams {
        ratio_tolerance: 0.001,
        min_votes: 2,
        ..Default::default()
    };
    let loose = TriangleParams {
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

    let low_min = TriangleParams {
        min_votes: 1,
        ..Default::default()
    };
    let high_min = TriangleParams {
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

    let config = TriangleParams {
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

    let config = TriangleParams::default();
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
