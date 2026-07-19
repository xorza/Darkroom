use crate::stacking::registration::triangle::tests::*;

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
