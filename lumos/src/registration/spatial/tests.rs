//! Tests for the spatial module (k-d tree).

use super::*;
use glam::DVec2;

/// Helper: collect radius search indices into a sorted Vec.
fn radius_search_indices(tree: &KdTree, query: DVec2, radius: f64) -> Vec<usize> {
    let mut buf = Vec::new();
    tree.radius_indices_into(query, radius, &mut buf);
    buf.sort();
    buf
}

// ============================================================================
// KdTree::build tests
// ============================================================================

#[test]
fn test_build_empty() {
    let tree = KdTree::build(&[]);
    assert!(tree.is_none());
}

#[test]
fn test_build_single_point() {
    let points = [DVec2::new(1.0, 2.0)];
    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 1);
    // get_point should return the original point
    let p = tree.get_point(0);
    assert_eq!(p.x, 1.0);
    assert_eq!(p.y, 2.0);
}

#[test]
fn test_build_preserves_all_points() {
    // Points are stored by original index regardless of internal permutation.
    let points = [
        DVec2::new(3.0, 1.0),
        DVec2::new(1.0, 3.0),
        DVec2::new(2.0, 2.0),
        DVec2::new(4.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 4);
    for (i, p) in points.iter().enumerate() {
        let stored = tree.get_point(i);
        assert_eq!(stored.x, p.x);
        assert_eq!(stored.y, p.y);
    }
}

// ============================================================================
// KdTree::k_nearest tests
// ============================================================================

#[test]
fn test_k_nearest_exact_distances() {
    // Layout (all on x-axis for easy hand-computation):
    //   idx 0: (0,0)  idx 1: (3,0)  idx 2: (7,0)  idx 3: (8,0)  idx 4: (15,0)
    // Query: (6,0)
    //   dist_sq to idx0: 36, idx1: 9, idx2: 1, idx3: 4, idx4: 81
    // k=3 nearest: idx2 (1), idx3 (4), idx1 (9)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(7.0, 0.0),
        DVec2::new(8.0, 0.0),
        DVec2::new(15.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(6.0, 0.0), 3);
    assert_eq!(neighbors.len(), 3);

    // Sorted by dist_sq: idx2=1.0, idx3=4.0, idx1=9.0
    assert_eq!(neighbors[0].index, 2);
    assert!((neighbors[0].dist_sq - 1.0).abs() < 1e-10);

    assert_eq!(neighbors[1].index, 3);
    assert!((neighbors[1].dist_sq - 4.0).abs() < 1e-10);

    assert_eq!(neighbors[2].index, 1);
    assert!((neighbors[2].dist_sq - 9.0).abs() < 1e-10);
}

#[test]
fn test_k_nearest_2d_distances() {
    // 2D layout:
    //   idx 0: (0,0)  idx 1: (3,4)  idx 2: (1,1)  idx 3: (6,8)
    // Query: (2, 2)
    //   dist_sq to idx0: (2-0)^2 + (2-0)^2 = 4+4 = 8
    //   dist_sq to idx1: (2-3)^2 + (2-4)^2 = 1+4 = 5
    //   dist_sq to idx2: (2-1)^2 + (2-1)^2 = 1+1 = 2
    //   dist_sq to idx3: (2-6)^2 + (2-8)^2 = 16+36 = 52
    // k=2 nearest: idx2 (2), idx1 (5)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 4.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(6.0, 8.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(2.0, 2.0), 2);
    assert_eq!(neighbors.len(), 2);

    assert_eq!(neighbors[0].index, 2);
    assert!((neighbors[0].dist_sq - 2.0).abs() < 1e-10);

    assert_eq!(neighbors[1].index, 1);
    assert!((neighbors[1].dist_sq - 5.0).abs() < 1e-10);
}

#[test]
fn test_k_nearest_finds_exact_point() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(5.0, 5.0), 1);
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].index, 2);
    assert_eq!(neighbors[0].dist_sq, 0.0);
}

#[test]
fn test_k_nearest_sorted_order() {
    // Points on x-axis: query at origin.
    //   idx 0: (0,0) dist_sq=0
    //   idx 1: (1,0) dist_sq=1
    //   idx 2: (2,0) dist_sq=4
    //   idx 3: (3,0) dist_sq=9
    //   idx 4: (10,0) dist_sq=100
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(10.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 3);
    assert_eq!(neighbors.len(), 3);

    assert_eq!(neighbors[0].index, 0);
    assert_eq!(neighbors[0].dist_sq, 0.0);
    assert_eq!(neighbors[1].index, 1);
    assert!((neighbors[1].dist_sq - 1.0).abs() < 1e-10);
    assert_eq!(neighbors[2].index, 2);
    assert!((neighbors[2].dist_sq - 4.0).abs() < 1e-10);
}

#[test]
fn test_k_nearest_more_than_available() {
    let points = [DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    // Request 10 but only 2 exist
    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 10);
    assert_eq!(neighbors.len(), 2);
    // Verify actual distances: idx0 at origin, idx1 at (1,1)
    // dist_sq to idx0: 0, dist_sq to idx1: 1+1=2
    assert_eq!(neighbors[0].index, 0);
    assert_eq!(neighbors[0].dist_sq, 0.0);
    assert_eq!(neighbors[1].index, 1);
    assert!((neighbors[1].dist_sq - 2.0).abs() < 1e-10);
}

#[test]
fn test_k_nearest_zero_k() {
    let points = [DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 0);
    assert!(neighbors.is_empty());
}

#[test]
fn test_k_nearest_negative_coordinates() {
    // idx 0: (-10,-10), idx 1: (-5,-5), idx 2: (0,0), idx 3: (5,5), idx 4: (10,10)
    // Query: (-7, -7)
    //   dist_sq to idx0: (-7+10)^2 + (-7+10)^2 = 9+9 = 18
    //   dist_sq to idx1: (-7+5)^2 + (-7+5)^2 = 4+4 = 8
    //   dist_sq to idx2: 49+49 = 98
    //   dist_sq to idx3: 144+144 = 288
    //   dist_sq to idx4: 289+289 = 578
    // k=2: idx1 (8), idx0 (18)
    let points = [
        DVec2::new(-10.0, -10.0),
        DVec2::new(-5.0, -5.0),
        DVec2::new(0.0, 0.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(-7.0, -7.0), 2);
    assert_eq!(neighbors.len(), 2);

    assert_eq!(neighbors[0].index, 1);
    assert!((neighbors[0].dist_sq - 8.0).abs() < 1e-10);

    assert_eq!(neighbors[1].index, 0);
    assert!((neighbors[1].dist_sq - 18.0).abs() < 1e-10);
}

#[test]
fn test_k_nearest_query_far_from_points() {
    // idx 0: (0,0), idx 1: (1,0), idx 2: (0,1), idx 3: (1,1)
    // Query: (1000, 1000)
    //   dist_sq to idx0: 1000^2 + 1000^2 = 2_000_000
    //   dist_sq to idx1: 999^2 + 1000^2 = 998001 + 1000000 = 1_998_001
    //   dist_sq to idx2: 1000^2 + 999^2 = 1000000 + 998001 = 1_998_001
    //   dist_sq to idx3: 999^2 + 999^2 = 998001 + 998001 = 1_996_002
    // k=2: idx3 (1_996_002), then idx1 or idx2 (1_998_001)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(1000.0, 1000.0), 2);
    assert_eq!(neighbors.len(), 2);

    assert_eq!(neighbors[0].index, 3);
    assert!((neighbors[0].dist_sq - 1_996_002.0).abs() < 1e-6);

    // idx1 and idx2 are equidistant; either is valid for [1]
    assert!((neighbors[1].dist_sq - 1_998_001.0).abs() < 1e-6);
    assert!(neighbors[1].index == 1 || neighbors[1].index == 2);
}

#[test]
fn test_k_nearest_duplicate_points() {
    // 3 duplicates at (5,5), 1 at (10,10)
    // Query: (5, 5)
    //   dist_sq to idx0,1,2: 0
    //   dist_sq to idx3: (5-10)^2 + (5-10)^2 = 50
    // k=3: all three duplicates at dist_sq=0
    let points = [
        DVec2::new(5.0, 5.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(5.0, 5.0), 3);
    assert_eq!(neighbors.len(), 3);

    // All three must be duplicates (indices 0, 1, or 2) with dist_sq=0
    let mut indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    indices.sort();
    assert_eq!(indices, vec![0, 1, 2]);
    for n in &neighbors {
        assert_eq!(n.dist_sq, 0.0);
    }
}

#[test]
fn test_k_nearest_all_identical_points() {
    let points: Vec<DVec2> = (0..5).map(|_| DVec2::new(7.0, 7.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(7.0, 7.0), 5);
    assert_eq!(neighbors.len(), 5);
    for n in &neighbors {
        assert_eq!(n.dist_sq, 0.0);
    }

    // All 5 indices should appear exactly once
    let mut indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    indices.sort();
    assert_eq!(indices, vec![0, 1, 2, 3, 4]);
}

#[test]
fn test_k_nearest_collinear_points_exact_distances() {
    // Collinear along y=x: idx0=(0,0), idx1=(1,1), idx2=(2,2), idx3=(3,3), idx4=(4,4)
    // Query: (2, 2)
    //   dist_sq to idx0: 4+4=8
    //   dist_sq to idx1: 1+1=2
    //   dist_sq to idx2: 0
    //   dist_sq to idx3: 1+1=2
    //   dist_sq to idx4: 4+4=8
    // k=3: idx2 (0), then idx1 and idx3 (both 2)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 2.0),
        DVec2::new(3.0, 3.0),
        DVec2::new(4.0, 4.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(2.0, 2.0), 3);
    assert_eq!(neighbors.len(), 3);

    assert_eq!(neighbors[0].index, 2);
    assert_eq!(neighbors[0].dist_sq, 0.0);

    // idx1 and idx3 are equidistant at dist_sq=2.0; order between them doesn't matter
    assert!((neighbors[1].dist_sq - 2.0).abs() < 1e-10);
    assert!((neighbors[2].dist_sq - 2.0).abs() < 1e-10);
    let mut tied_indices: Vec<usize> = vec![neighbors[1].index, neighbors[2].index];
    tied_indices.sort();
    assert_eq!(tied_indices, vec![1, 3]);
}

#[test]
fn test_k_nearest_clustered_points() {
    // Cluster 1 near origin: idx 0..5 at (0, 0), (0.1, 0.1), (0.2, 0.2), (0.3, 0.3), (0.4, 0.4)
    // Cluster 2 near (100,100): idx 5..10 at (100, 100), (100.1, 100.1), ...
    // Query at (0,0), k=5: all from cluster 1
    //   dist_sq to idx0: 0
    //   dist_sq to idx1: 0.01+0.01 = 0.02
    //   dist_sq to idx2: 0.04+0.04 = 0.08
    //   dist_sq to idx3: 0.09+0.09 = 0.18
    //   dist_sq to idx4: 0.16+0.16 = 0.32
    let mut points = Vec::new();
    for i in 0..5 {
        points.push(DVec2::new(i as f64 * 0.1, i as f64 * 0.1));
    }
    for i in 0..5 {
        points.push(DVec2::new(100.0 + i as f64 * 0.1, 100.0 + i as f64 * 0.1));
    }

    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 5);
    assert_eq!(neighbors.len(), 5);
    assert_eq!(neighbors[0].index, 0);
    assert_eq!(neighbors[0].dist_sq, 0.0);
    assert_eq!(neighbors[1].index, 1);
    assert!((neighbors[1].dist_sq - 0.02).abs() < 1e-10);
    assert_eq!(neighbors[2].index, 2);
    assert!((neighbors[2].dist_sq - 0.08).abs() < 1e-10);
    assert_eq!(neighbors[3].index, 3);
    assert!((neighbors[3].dist_sq - 0.18).abs() < 1e-10);
    assert_eq!(neighbors[4].index, 4);
    assert!((neighbors[4].dist_sq - 0.32).abs() < 1e-10);

    // Query at (100, 100), k=5: all from cluster 2
    //   dist_sq to idx5: 0
    //   dist_sq to idx6: 0.02
    //   dist_sq to idx7: 0.08
    //   dist_sq to idx8: 0.18
    //   dist_sq to idx9: 0.32
    let neighbors = tree.k_nearest(DVec2::new(100.0, 100.0), 5);
    assert_eq!(neighbors.len(), 5);
    assert_eq!(neighbors[0].index, 5);
    assert_eq!(neighbors[0].dist_sq, 0.0);
    assert_eq!(neighbors[1].index, 6);
    assert!((neighbors[1].dist_sq - 0.02).abs() < 1e-10);
    assert_eq!(neighbors[4].index, 9);
    assert!((neighbors[4].dist_sq - 0.32).abs() < 1e-10);
}

#[test]
fn test_k_nearest_with_large_k_uses_large_heap() {
    // 50 points on x-axis: idx i at (i, 0)
    // Query: (0, 0), k = SMALL_HEAP_CAPACITY + 5 (=37)
    // The i-th nearest has dist_sq = i^2. Results should be idx 0..37 in order.
    let points: Vec<DVec2> = (0..50).map(|i| DVec2::new(i as f64, 0.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    let k = SMALL_HEAP_CAPACITY + 5; // 37
    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), k);
    assert_eq!(neighbors.len(), k);

    for (rank, n) in neighbors.iter().enumerate() {
        // rank-th nearest should be idx=rank at dist_sq=rank^2
        assert_eq!(n.index, rank);
        let expected_dist_sq = (rank as f64) * (rank as f64);
        assert!((n.dist_sq - expected_dist_sq).abs() < 1e-10);
    }
}

// ============================================================================
// KdTree::nearest_one tests
// ============================================================================

#[test]
fn test_nearest_one_exact_match() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let nn = tree.nearest_one(DVec2::new(5.0, 5.0)).unwrap();
    assert_eq!(nn.index, 2);
    assert_eq!(nn.dist_sq, 0.0);
}

#[test]
fn test_nearest_one_single_point() {
    // Single point at (3, 7). Query at origin.
    // dist_sq = 3^2 + 7^2 = 9 + 49 = 58
    let points = [DVec2::new(3.0, 7.0)];
    let tree = KdTree::build(&points).unwrap();
    let nn = tree.nearest_one(DVec2::new(0.0, 0.0)).unwrap();
    assert_eq!(nn.index, 0);
    assert!((nn.dist_sq - 58.0).abs() < 1e-10);
}

#[test]
fn test_nearest_one_equidistant() {
    // idx 0: (3,4), idx 1: (5,5)
    // Query: (4, 4.5)
    //   dist_sq to idx0: (4-3)^2 + (4.5-4)^2 = 1 + 0.25 = 1.25
    //   dist_sq to idx1: (4-5)^2 + (4.5-5)^2 = 1 + 0.25 = 1.25
    // Both equidistant — either is valid
    let points = [DVec2::new(3.0, 4.0), DVec2::new(5.0, 5.0)];
    let tree = KdTree::build(&points).unwrap();

    let nn = tree.nearest_one(DVec2::new(4.0, 4.5)).unwrap();
    assert!((nn.dist_sq - 1.25).abs() < 1e-10);
    assert!(nn.index == 0 || nn.index == 1);
}

#[test]
fn test_nearest_one_agrees_with_k_nearest_1() {
    // Verify nearest_one returns the same result as k_nearest(q, 1)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(3.0, 4.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let query = DVec2::new(7.0, 8.0);
    // dist_sq to idx0: 49+64=113, idx1: 9+4=13, idx2: 4+9=13, idx3: 16+16=32
    // idx1 and idx2 tie at 13; nearest_one and k_nearest should agree
    let nn = tree.nearest_one(query).unwrap();
    let kn = tree.k_nearest(query, 1);
    assert_eq!(nn.index, kn[0].index);
    assert!((nn.dist_sq - kn[0].dist_sq).abs() < 1e-10);
    assert!((nn.dist_sq - 13.0).abs() < 1e-10);
}

#[test]
fn test_nearest_one_empty_tree_not_possible() {
    // KdTree::build returns None for empty input, so nearest_one on an empty
    // tree can't happen through the public API. This test documents that
    // build(&[]) returns None.
    assert!(KdTree::build(&[]).is_none());
}

// ============================================================================
// KdTree::radius_indices_into tests
// ============================================================================

#[test]
fn test_radius_finds_correct_points() {
    // idx 0: (0,0), idx 1: (1,0), idx 2: (0,1), idx 3: (5,5), idx 4: (10,10)
    // Query: (0,0), radius: 2.0, radius_sq = 4.0
    //   dist_sq to idx0: 0 <= 4 => included
    //   dist_sq to idx1: 1 <= 4 => included
    //   dist_sq to idx2: 1 <= 4 => included
    //   dist_sq to idx3: 50 > 4 => excluded
    //   dist_sq to idx4: 200 > 4 => excluded
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 2.0);
    assert_eq!(indices, vec![0, 1, 2]);
}

#[test]
fn test_radius_empty_result() {
    // All points far from query
    // idx 0: (0,0), idx 1: (10,10)
    // Query: (5, 5), radius: 1.0, radius_sq = 1.0
    //   dist_sq to idx0: 25+25=50 > 1
    //   dist_sq to idx1: 25+25=50 > 1
    let points = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 10.0)];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(5.0, 5.0), 1.0);
    assert!(indices.is_empty());
}

#[test]
fn test_radius_all_points_included() {
    // idx 0: (0,0), idx 1: (1,0), idx 2: (0,1), idx 3: (1,1)
    // Query: (0.5, 0.5), radius: 10.0
    // All within radius
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.5, 0.5), 10.0);
    assert_eq!(indices, vec![0, 1, 2, 3]);
}

#[test]
fn test_radius_boundary_inclusion() {
    // idx 0: (0,0), idx 1: (1,0), idx 2: (2,0)
    // Query: (0, 0), radius: 1.0, radius_sq = 1.0
    //   dist_sq to idx0: 0 <= 1 => included
    //   dist_sq to idx1: 1 <= 1 => included (boundary: dist_sq == radius_sq)
    //   dist_sq to idx2: 4 > 1 => excluded
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 1.0);
    assert_eq!(indices, vec![0, 1]);
}

#[test]
fn test_radius_zero() {
    // radius=0 means only exact matches (dist_sq=0 <= 0)
    let points = [DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 0.0);
    assert_eq!(indices, vec![0]);
}

#[test]
fn test_radius_negative_coordinates() {
    // idx 0: (-3, -4), idx 1: (0, 0), idx 2: (3, 4)
    // Query: (-2, -3), radius: 2.0, radius_sq = 4.0
    //   dist_sq to idx0: (-2+3)^2 + (-3+4)^2 = 1+1 = 2 <= 4 => included
    //   dist_sq to idx1: 4+9 = 13 > 4 => excluded
    //   dist_sq to idx2: 25+49 = 74 > 4 => excluded
    let points = [
        DVec2::new(-3.0, -4.0),
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 4.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(-2.0, -3.0), 2.0);
    assert_eq!(indices, vec![0]);
}

#[test]
fn test_radius_buffer_reuse_clears() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let mut buf = Vec::new();

    // First query near origin: should find idx 0 and 1
    tree.radius_indices_into(DVec2::new(0.0, 0.0), 2.0, &mut buf);
    buf.sort();
    assert_eq!(buf, vec![0, 1]);

    // Second query near (10,10): should find only idx 2 (buffer cleared)
    tree.radius_indices_into(DVec2::new(10.0, 10.0), 0.5, &mut buf);
    assert_eq!(buf, vec![2]);
}

#[test]
fn test_radius_different_radii_different_results() {
    // idx 0: (0,0), idx 1: (2,0), idx 2: (5,0)
    // Query: (0,0)
    //   radius=1.5: radius_sq=2.25 → only idx0 (dist_sq=0)
    //   radius=3.0: radius_sq=9.0  → idx0 (0) + idx1 (4)
    //   radius=6.0: radius_sq=36.0 → all three: idx0 (0), idx1 (4), idx2 (25)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(5.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let r1 = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 1.5);
    let r2 = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 3.0);
    let r3 = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 6.0);

    assert_eq!(r1, vec![0]);
    assert_eq!(r2, vec![0, 1]);
    assert_eq!(r3, vec![0, 1, 2]);
}

// ============================================================================
// KdTree::get_point tests
// ============================================================================

#[test]
fn test_get_point_returns_original_coordinates() {
    let points = [
        DVec2::new(3.125, 2.71),
        DVec2::new(-1.0, 42.0),
        DVec2::new(0.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    for (i, p) in points.iter().enumerate() {
        let stored = tree.get_point(i);
        assert_eq!(stored.x, p.x);
        assert_eq!(stored.y, p.y);
    }
}

// ============================================================================
// BoundedMaxHeap tests
// ============================================================================

#[test]
fn test_heap_small_variant_selection() {
    let heap = BoundedMaxHeap::new(5);
    assert!(matches!(heap, BoundedMaxHeap::Small { .. }));

    let heap = BoundedMaxHeap::new(SMALL_HEAP_CAPACITY);
    assert!(matches!(heap, BoundedMaxHeap::Small { .. }));

    let heap = BoundedMaxHeap::new(SMALL_HEAP_CAPACITY + 1);
    assert!(matches!(heap, BoundedMaxHeap::Large { .. }));
}

#[test]
fn test_heap_empty_state() {
    let heap_small = BoundedMaxHeap::new(5);
    assert!(!heap_small.is_full());
    assert_eq!(heap_small.max_distance(), f64::INFINITY);

    let heap_large = BoundedMaxHeap::new(50);
    assert!(!heap_large.is_full());
    assert_eq!(heap_large.max_distance(), f64::INFINITY);
}

#[test]
fn test_heap_small_push_and_eviction() {
    let mut heap = BoundedMaxHeap::new(3);

    // Push 3 items: dist_sq = 10, 5, 15
    heap.push(Neighbor {
        index: 0,
        dist_sq: 10.0,
    });
    heap.push(Neighbor {
        index: 1,
        dist_sq: 5.0,
    });
    heap.push(Neighbor {
        index: 2,
        dist_sq: 15.0,
    });

    assert!(heap.is_full());
    // Max-heap root should be the largest: 15.0
    assert!((heap.max_distance() - 15.0).abs() < 1e-10);

    // Push smaller item (2.0) — should evict 15.0
    heap.push(Neighbor {
        index: 3,
        dist_sq: 2.0,
    });
    // New max should be 10.0
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    // Push larger item (20.0) — should be rejected
    heap.push(Neighbor {
        index: 4,
        dist_sq: 20.0,
    });
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    // Final contents: dist_sq = {10, 5, 2}, indices = {0, 1, 3}
    let result = heap.into_vec();
    assert_eq!(result.len(), 3);
    let mut dist_sqs: Vec<u64> = result.iter().map(|n| n.dist_sq.to_bits()).collect();
    dist_sqs.sort();
    let expected: Vec<u64> = [2.0_f64, 5.0, 10.0].iter().map(|d| d.to_bits()).collect();
    assert_eq!(dist_sqs, expected);

    let mut indices: Vec<usize> = result.iter().map(|n| n.index).collect();
    indices.sort();
    assert_eq!(indices, vec![0, 1, 3]);
}

#[test]
fn test_heap_large_push_and_eviction() {
    let capacity = SMALL_HEAP_CAPACITY + 5; // 37
    let mut heap = BoundedMaxHeap::new(capacity);
    assert!(matches!(heap, BoundedMaxHeap::Large { .. }));

    // Push capacity items with dist_sq = capacity, capacity-1, ..., 1
    for i in 0..capacity {
        heap.push(Neighbor {
            index: i,
            dist_sq: (capacity - i) as f64,
        });
    }

    assert!(heap.is_full());
    // Max should be capacity (=37)
    assert!((heap.max_distance() - capacity as f64).abs() < 1e-10);

    // Push 0.5 — should evict the max (37.0)
    heap.push(Neighbor {
        index: 100,
        dist_sq: 0.5,
    });
    // New max should be capacity-1 = 36
    assert!((heap.max_distance() - (capacity - 1) as f64).abs() < 1e-10);

    let result = heap.into_vec();
    assert_eq!(result.len(), capacity);
    // Should contain 0.5 and 1..36
    let has_half = result.iter().any(|n| (n.dist_sq - 0.5).abs() < 1e-10);
    assert!(has_half);
    // Should NOT contain the evicted max (37.0)
    let has_max = result
        .iter()
        .any(|n| (n.dist_sq - capacity as f64).abs() < 1e-10);
    assert!(!has_max);
}

#[test]
fn test_heap_capacity_one() {
    // Capacity 1: only keeps the single smallest
    let mut heap = BoundedMaxHeap::new(1);

    heap.push(Neighbor {
        index: 0,
        dist_sq: 10.0,
    });
    assert!(heap.is_full());
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    // Push smaller — should replace
    heap.push(Neighbor {
        index: 1,
        dist_sq: 3.0,
    });
    assert!((heap.max_distance() - 3.0).abs() < 1e-10);

    // Push larger — should be rejected
    heap.push(Neighbor {
        index: 2,
        dist_sq: 50.0,
    });
    assert!((heap.max_distance() - 3.0).abs() < 1e-10);

    let result = heap.into_vec();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].index, 1);
    assert!((result[0].dist_sq - 3.0).abs() < 1e-10);
}

// ============================================================================
// Cross-method consistency tests
// ============================================================================

#[test]
fn test_k_nearest_and_radius_agree() {
    // 5 points on x-axis. Query at origin, radius=4.5 (radius_sq=20.25).
    // idx 0: (0,0) dist_sq=0, idx 1: (2,0) dist_sq=4, idx 2: (4,0) dist_sq=16,
    // idx 3: (6,0) dist_sq=36, idx 4: (8,0) dist_sq=64
    // Radius should find: idx 0 (0), idx 1 (4), idx 2 (16) — all <= 20.25
    // k_nearest(3) from origin should find the same three
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(4.0, 0.0),
        DVec2::new(6.0, 0.0),
        DVec2::new(8.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();
    let query = DVec2::new(0.0, 0.0);

    let radius_result = radius_search_indices(&tree, query, 4.5);
    assert_eq!(radius_result, vec![0, 1, 2]);

    let knn_result = tree.k_nearest(query, 3);
    let mut knn_indices: Vec<usize> = knn_result.iter().map(|n| n.index).collect();
    knn_indices.sort();
    assert_eq!(knn_indices, vec![0, 1, 2]);
}

#[test]
fn test_horizontal_line_exact_distances() {
    // 10 points on x-axis: idx i at (10*i, 0), i=0..10
    // Query: (45, 0)
    //   dist_sq to idx4 (40,0): (45-40)^2 = 25
    //   dist_sq to idx5 (50,0): (45-50)^2 = 25
    //   dist_sq to idx3 (30,0): (45-30)^2 = 225
    // k=2: idx4 and idx5, both at dist_sq=25
    let points: Vec<DVec2> = (0..10).map(|i| DVec2::new(i as f64 * 10.0, 0.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(45.0, 0.0), 2);
    assert_eq!(neighbors.len(), 2);
    assert!((neighbors[0].dist_sq - 25.0).abs() < 1e-10);
    assert!((neighbors[1].dist_sq - 25.0).abs() < 1e-10);
    let mut indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    indices.sort();
    assert_eq!(indices, vec![4, 5]);
}

#[test]
fn test_vertical_line_exact_distances() {
    // 10 points on y-axis: idx i at (0, 10*i), i=0..10
    // Query: (0, 45)
    //   dist_sq to idx4 (0,40): (45-40)^2 = 25
    //   dist_sq to idx5 (0,50): (45-50)^2 = 25
    // k=2: idx4 and idx5, both at dist_sq=25
    let points: Vec<DVec2> = (0..10).map(|i| DVec2::new(0.0, i as f64 * 10.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 45.0), 2);
    assert_eq!(neighbors.len(), 2);
    assert!((neighbors[0].dist_sq - 25.0).abs() < 1e-10);
    assert!((neighbors[1].dist_sq - 25.0).abs() < 1e-10);
    let mut indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    indices.sort();
    assert_eq!(indices, vec![4, 5]);
}

#[test]
fn test_large_coordinates() {
    // idx 0: (1024.5, 768.3), idx 1: (2048.1, 1536.7), idx 2: (512.9, 384.2), idx 3: (3072.0, 2304.5)
    // Query: (1024.5, 768.3) — exact match with idx 0
    // k=2: idx 0 (dist_sq=0), next closest:
    //   dist_sq to idx1: (1024.5-2048.1)^2 + (768.3-1536.7)^2 = 1047564.96 + 590790.76 = 1638355.72
    //   dist_sq to idx2: (1024.5-512.9)^2 + (768.3-384.2)^2 = 261793.56 + 147464.81 = 409258.37  (closest)
    //   dist_sq to idx3: (1024.5-3072)^2 + (768.3-2304.5)^2 = 4197556.25 + 2361564.84 = 6559121.09
    let points = [
        DVec2::new(1024.5, 768.3),
        DVec2::new(2048.1, 1536.7),
        DVec2::new(512.9, 384.2),
        DVec2::new(3072.0, 2304.5),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(1024.5, 768.3), 2);
    assert_eq!(neighbors.len(), 2);
    assert_eq!(neighbors[0].index, 0);
    assert_eq!(neighbors[0].dist_sq, 0.0);
    assert_eq!(neighbors[1].index, 2);
    // (1024.5-512.9)^2 + (768.3-384.2)^2 = 511.6^2 + 384.1^2 = 261734.56 + 147532.81 = 409267.37
    let dx = 1024.5 - 512.9;
    let dy = 768.3 - 384.2;
    let expected = dx * dx + dy * dy;
    assert!((neighbors[1].dist_sq - expected).abs() < 1e-6);
}
