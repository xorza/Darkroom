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

#[test]
fn test_kdtree_build_empty() {
    let tree = KdTree::build(&[]);
    assert!(tree.is_none());
}

#[test]
fn test_kdtree_build_single_point() {
    let points = vec![DVec2::new(1.0, 2.0)];
    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 1);
}

#[test]
fn test_kdtree_build_multiple_points() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 4);
}

#[test]
fn test_kdtree_k_nearest_basic() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.5, 0.5),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.5, 0.5), 3);
    assert_eq!(neighbors.len(), 3);

    assert_eq!(neighbors[0].index, 4); // Center point itself
    assert!(neighbors[0].dist_sq < 0.001);
}

#[test]
fn test_kdtree_k_nearest_finds_exact_point() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(5.0, 5.0), 1);
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].index, 2);
    assert!(neighbors[0].dist_sq < 1e-10);
}

#[test]
fn test_kdtree_k_nearest_order() {
    let points = vec![
        DVec2::new(0.0, 0.0),  // index 0, dist_sq = 0
        DVec2::new(1.0, 0.0),  // index 1, dist_sq = 1
        DVec2::new(2.0, 0.0),  // index 2, dist_sq = 4
        DVec2::new(3.0, 0.0),  // index 3, dist_sq = 9
        DVec2::new(10.0, 0.0), // index 4, dist_sq = 100
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 3);
    assert_eq!(neighbors.len(), 3);

    assert!(neighbors[0].dist_sq <= neighbors[1].dist_sq);
    assert!(neighbors[1].dist_sq <= neighbors[2].dist_sq);

    assert_eq!(neighbors[0].index, 0);
    assert!(neighbors[0].dist_sq < 1e-10);
    assert_eq!(neighbors[1].index, 1);
    assert!((neighbors[1].dist_sq - 1.0).abs() < 1e-10);
    assert_eq!(neighbors[2].index, 2);
    assert!((neighbors[2].dist_sq - 4.0).abs() < 1e-10);
}

#[test]
fn test_kdtree_k_nearest_more_than_available() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 10);
    assert_eq!(neighbors.len(), 2);
}

#[test]
fn test_kdtree_radius_indices_basic() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 2.0);

    // Should find points 0, 1, 2 (all within distance 2)
    assert_eq!(indices.len(), 3);
    assert!(indices.contains(&0));
    assert!(indices.contains(&1));
    assert!(indices.contains(&2));
}

#[test]
fn test_kdtree_radius_indices_empty_result() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(10.0, 10.0)];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(5.0, 5.0), 1.0);
    assert!(indices.is_empty());
}

#[test]
fn test_kdtree_radius_indices_all_points() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.5, 0.5), 10.0);
    assert_eq!(indices.len(), 4);
}

#[test]
fn test_kdtree_with_large_coordinates() {
    let points = vec![
        DVec2::new(1024.5, 768.3),
        DVec2::new(2048.1, 1536.7),
        DVec2::new(512.9, 384.2),
        DVec2::new(3072.0, 2304.5),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(1024.5, 768.3), 2);
    assert_eq!(neighbors.len(), 2);
    assert_eq!(neighbors[0].index, 0);
}

#[test]
fn test_kdtree_with_collinear_points() {
    let points = vec![
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
}

#[test]
fn test_kdtree_with_clustered_points() {
    let mut points = Vec::new();

    // Cluster 1 around (0, 0)
    for i in 0..5 {
        points.push(DVec2::new(i as f64 * 0.1, i as f64 * 0.1));
    }

    // Cluster 2 around (100, 100)
    for i in 0..5 {
        points.push(DVec2::new(100.0 + i as f64 * 0.1, 100.0 + i as f64 * 0.1));
    }

    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 5);
    for n in neighbors {
        assert!(n.index < 5, "Should only find points from cluster 1");
    }

    let neighbors = tree.k_nearest(DVec2::new(100.0, 100.0), 5);
    for n in neighbors {
        assert!(n.index >= 5, "Should only find points from cluster 2");
    }
}

// ============================================================================
// Additional edge case tests
// ============================================================================

#[test]
fn test_kdtree_duplicate_points() {
    let points = vec![
        DVec2::new(5.0, 5.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    assert_eq!(tree.len(), 4);

    let neighbors = tree.k_nearest(DVec2::new(5.0, 5.0), 3);
    assert_eq!(neighbors.len(), 3);

    for n in &neighbors {
        if n.index < 3 {
            assert!(
                n.dist_sq < 1e-10,
                "Duplicate point should have zero distance"
            );
        }
    }
}

#[test]
fn test_kdtree_many_points() {
    let mut points = Vec::with_capacity(1000);
    for y in 0..32 {
        for x in 0..32 {
            if points.len() < 1000 {
                points.push(DVec2::new(x as f64 * 10.0, y as f64 * 10.0));
            }
        }
    }

    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 1000);

    let neighbors = tree.k_nearest(DVec2::new(155.0, 155.0), 10);
    assert_eq!(neighbors.len(), 10);

    for n in &neighbors {
        assert!(
            n.dist_sq < 1000.0,
            "Neighbor too far: {} (squared distance)",
            n.dist_sq
        );
    }
}

#[test]
fn test_kdtree_horizontal_line() {
    let points: Vec<DVec2> = (0..10).map(|i| DVec2::new(i as f64 * 10.0, 0.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    assert_eq!(tree.len(), 10);

    let neighbors = tree.k_nearest(DVec2::new(45.0, 0.0), 3);
    assert_eq!(neighbors.len(), 3);

    let indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    assert!(indices.contains(&4) || indices.contains(&5));
}

#[test]
fn test_kdtree_vertical_line() {
    let points: Vec<DVec2> = (0..10).map(|i| DVec2::new(0.0, i as f64 * 10.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    assert_eq!(tree.len(), 10);

    let neighbors = tree.k_nearest(DVec2::new(0.0, 45.0), 3);
    assert_eq!(neighbors.len(), 3);

    let indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    assert!(indices.contains(&4) || indices.contains(&5));
}

#[test]
fn test_kdtree_all_identical_points() {
    let points: Vec<DVec2> = (0..5).map(|_| DVec2::new(7.0, 7.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    assert_eq!(tree.len(), 5);

    let neighbors = tree.k_nearest(DVec2::new(7.0, 7.0), 5);
    assert_eq!(neighbors.len(), 5);

    for n in &neighbors {
        assert!(
            n.dist_sq < 1e-10,
            "All points at same location should have zero distance"
        );
    }
}

#[test]
fn test_kdtree_radius_indices_boundary() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0), // Exactly at radius 1
        DVec2::new(2.0, 0.0), // Beyond radius 1
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 1.0);

    // Should find at least point 0 (distance 0); point 1 is at boundary (dist_sq == radius_sq)
    assert!(!indices.is_empty());
    assert!(indices.contains(&0));
    // Point at exactly radius 1: dist_sq=1.0 <= radius_sq=1.0, so included
    assert!(indices.contains(&1));
    assert!(!indices.contains(&2));
}

#[test]
fn test_kdtree_k_nearest_zero() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 0);
    assert!(neighbors.is_empty());
}

#[test]
fn test_kdtree_radius_indices_zero_radius() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 0.0);
    // dist_sq=0.0 <= radius_sq=0.0, so exact match included
    assert_eq!(indices.len(), 1);
    assert!(indices.contains(&0));
}

#[test]
fn test_kdtree_query_far_from_points() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(1000.0, 1000.0), 2);
    assert_eq!(neighbors.len(), 2);

    for n in &neighbors {
        assert!(n.dist_sq > 1_000_000.0, "Distance should be very large");
    }
}

#[test]
fn test_kdtree_negative_coordinates() {
    let points = vec![
        DVec2::new(-10.0, -10.0),
        DVec2::new(-5.0, -5.0),
        DVec2::new(0.0, 0.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(-7.0, -7.0), 2);
    assert_eq!(neighbors.len(), 2);

    let indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    assert!(indices.contains(&0) || indices.contains(&1));
}

#[test]
fn test_kdtree_radius_indices_buffer_reuse() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let mut buf = Vec::new();

    // First query: near origin
    tree.radius_indices_into(DVec2::new(0.0, 0.0), 2.0, &mut buf);
    assert_eq!(buf.len(), 2);

    // Second query: reuses buffer, should be cleared
    tree.radius_indices_into(DVec2::new(10.0, 10.0), 0.5, &mut buf);
    assert_eq!(buf.len(), 1);
    assert!(buf.contains(&2));
}

// ============================================================================
// BoundedMaxHeap tests
// ============================================================================

#[test]
fn test_bounded_max_heap_small() {
    let mut heap = BoundedMaxHeap::new(5);

    assert!(matches!(heap, BoundedMaxHeap::Small { .. }));

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
    heap.push(Neighbor {
        index: 3,
        dist_sq: 3.0,
    });
    heap.push(Neighbor {
        index: 4,
        dist_sq: 8.0,
    });

    assert!(heap.is_full());
    assert!((heap.max_distance() - 15.0).abs() < 1e-10);

    // Push a smaller item - should replace the max
    heap.push(Neighbor {
        index: 5,
        dist_sq: 2.0,
    });
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    // Push a larger item - should be rejected
    heap.push(Neighbor {
        index: 6,
        dist_sq: 20.0,
    });
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    let result = heap.into_vec();
    assert_eq!(result.len(), 5);
}

#[test]
fn test_bounded_max_heap_large() {
    let capacity = SMALL_HEAP_CAPACITY + 10;
    let mut heap = BoundedMaxHeap::new(capacity);

    assert!(matches!(heap, BoundedMaxHeap::Large { .. }));

    for i in 0..capacity {
        heap.push(Neighbor {
            index: i,
            dist_sq: (capacity - i) as f64,
        });
    }

    assert!(heap.is_full());
    assert!((heap.max_distance() - capacity as f64).abs() < 1e-10);

    heap.push(Neighbor {
        index: 100,
        dist_sq: 0.5,
    });
    assert!((heap.max_distance() - (capacity - 1) as f64).abs() < 1e-10);

    let result = heap.into_vec();
    assert_eq!(result.len(), capacity);
}

#[test]
fn test_bounded_max_heap_boundary() {
    let heap_at_boundary = BoundedMaxHeap::new(SMALL_HEAP_CAPACITY);
    assert!(matches!(heap_at_boundary, BoundedMaxHeap::Small { .. }));

    let heap_above = BoundedMaxHeap::new(SMALL_HEAP_CAPACITY + 1);
    assert!(matches!(heap_above, BoundedMaxHeap::Large { .. }));
}

#[test]
fn test_kdtree_k_nearest_small_k() {
    let mut points = Vec::new();
    for i in 0..100 {
        points.push(DVec2::new(i as f64, (i * i % 50) as f64));
    }

    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(50.0, 25.0), 10);
    assert_eq!(neighbors.len(), 10);

    for i in 1..neighbors.len() {
        assert!(neighbors[i - 1].dist_sq <= neighbors[i].dist_sq);
    }
}

#[test]
fn test_kdtree_k_nearest_large_k() {
    let mut points = Vec::new();
    for i in 0..100 {
        points.push(DVec2::new(i as f64, (i * i % 50) as f64));
    }

    let tree = KdTree::build(&points).unwrap();

    let k = SMALL_HEAP_CAPACITY + 10;
    let neighbors = tree.k_nearest(DVec2::new(50.0, 25.0), k);
    assert_eq!(neighbors.len(), k);

    for i in 1..neighbors.len() {
        assert!(neighbors[i - 1].dist_sq <= neighbors[i].dist_sq);
    }
}

#[test]
fn test_bounded_max_heap_empty() {
    let heap_small = BoundedMaxHeap::new(5);
    assert!(!heap_small.is_full());
    assert_eq!(heap_small.max_distance(), f64::INFINITY);

    let heap_large = BoundedMaxHeap::new(50);
    assert!(!heap_large.is_full());
    assert_eq!(heap_large.max_distance(), f64::INFINITY);
}
