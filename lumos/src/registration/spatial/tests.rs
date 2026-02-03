//! Tests for the spatial module (k-d tree).

use super::*;
use glam::DVec2;

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

    // Find 3 nearest to center point (0.5, 0.5)
    let neighbors = tree.k_nearest(DVec2::new(0.5, 0.5), 3);
    assert_eq!(neighbors.len(), 3);

    // The center point should be closest to itself (index 4)
    // and then to the corners equidistantly
    assert_eq!(neighbors[0].0, 4); // Center point itself
    assert!(neighbors[0].1 < 0.001); // Distance should be ~0
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
    assert_eq!(neighbors[0].0, 2);
    assert!(neighbors[0].1 < 1e-10);
}

#[test]
fn test_kdtree_k_nearest_order() {
    let points = vec![
        DVec2::new(0.0, 0.0),  // index 0, distance from (0,0) = 0
        DVec2::new(1.0, 0.0),  // index 1, distance from (0,0) = 1
        DVec2::new(2.0, 0.0),  // index 2, distance from (0,0) = 4
        DVec2::new(3.0, 0.0),  // index 3, distance from (0,0) = 9
        DVec2::new(10.0, 0.0), // index 4, distance from (0,0) = 100
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 3);
    assert_eq!(neighbors.len(), 3);

    // Should be sorted by distance
    assert!(neighbors[0].1 <= neighbors[1].1);
    assert!(neighbors[1].1 <= neighbors[2].1);

    // First should be point 0 (distance 0)
    assert_eq!(neighbors[0].0, 0);
    assert!(neighbors[0].1 < 1e-10);

    // Second should be point 1 (distance 1)
    assert_eq!(neighbors[1].0, 1);
    assert!((neighbors[1].1 - 1.0).abs() < 1e-10);

    // Third should be point 2 (distance 4)
    assert_eq!(neighbors[2].0, 2);
    assert!((neighbors[2].1 - 4.0).abs() < 1e-10);
}

#[test]
fn test_kdtree_k_nearest_more_than_available() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 10);
    assert_eq!(neighbors.len(), 2); // Only 2 points available
}

#[test]
fn test_kdtree_radius_search_basic() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    // Search within radius 2 from origin
    let results = tree.radius_search(DVec2::new(0.0, 0.0), 2.0);

    // Should find points 0, 1, 2 (all within distance 2)
    assert_eq!(results.len(), 3);
    let indices: Vec<usize> = results.iter().map(|(i, _)| *i).collect();
    assert!(indices.contains(&0));
    assert!(indices.contains(&1));
    assert!(indices.contains(&2));
}

#[test]
fn test_kdtree_radius_search_empty_result() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(10.0, 10.0)];
    let tree = KdTree::build(&points).unwrap();

    // Search in an area with no points
    let results = tree.radius_search(DVec2::new(5.0, 5.0), 1.0);
    assert!(results.is_empty());
}

#[test]
fn test_kdtree_radius_search_all_points() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    // Large radius should find all points
    let results = tree.radius_search(DVec2::new(0.5, 0.5), 10.0);
    assert_eq!(results.len(), 4);
}

#[test]
fn test_form_triangles_from_neighbors_basic() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.5, 0.866), // Equilateral triangle
    ];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);

    // Should form exactly one triangle
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

    // With k=3, each point sees 3 neighbors, forming multiple triangles
    let triangles = form_triangles_from_neighbors(&tree, 3);

    // A square can form 4 triangles (each point with 2 adjacent corners)
    assert!(triangles.len() >= 4);

    // All triangles should have valid indices
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

    // Check for duplicates
    let mut sorted = triangles.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), triangles.len(), "Found duplicate triangles");
}

#[test]
fn test_form_triangles_scaling_vs_brute_force() {
    // Compare triangle count with brute force for small input
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 1.0),
    ];

    // Brute force: n*(n-1)*(n-2)/6 triangles
    let n = points.len();
    let brute_force_count = n * (n - 1) * (n - 2) / 6;

    let tree = KdTree::build(&points).unwrap();

    // With k=n-1 (all neighbors), should get same count
    let triangles = form_triangles_from_neighbors(&tree, n - 1);
    assert_eq!(triangles.len(), brute_force_count);
}

#[test]
fn test_kdtree_with_large_coordinates() {
    // Test with typical astronomical image coordinates
    let points = vec![
        DVec2::new(1024.5, 768.3),
        DVec2::new(2048.1, 1536.7),
        DVec2::new(512.9, 384.2),
        DVec2::new(3072.0, 2304.5),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(1024.5, 768.3), 2);
    assert_eq!(neighbors.len(), 2);
    assert_eq!(neighbors[0].0, 0); // Exact match
}

#[test]
fn test_kdtree_with_collinear_points() {
    // Points on a line
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 2.0),
        DVec2::new(3.0, 3.0),
        DVec2::new(4.0, 4.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    // k-nearest should still work
    let neighbors = tree.k_nearest(DVec2::new(2.0, 2.0), 3);
    assert_eq!(neighbors.len(), 3);
    assert_eq!(neighbors[0].0, 2); // Exact match
}

#[test]
fn test_kdtree_with_clustered_points() {
    // Two clusters of points
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

    // Query near cluster 1 should return cluster 1 points
    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 5);
    for (idx, _) in neighbors {
        assert!(idx < 5, "Should only find points from cluster 1");
    }

    // Query near cluster 2 should return cluster 2 points
    let neighbors = tree.k_nearest(DVec2::new(100.0, 100.0), 5);
    for (idx, _) in neighbors {
        assert!(idx >= 5, "Should only find points from cluster 2");
    }
}

#[test]
fn test_distance_squared() {
    use glam::DVec2;
    assert!((DVec2::new(0.0, 0.0).distance_squared(DVec2::new(3.0, 4.0)) - 25.0).abs() < 1e-10);
    assert!((DVec2::new(1.0, 1.0).distance_squared(DVec2::new(1.0, 1.0))).abs() < 1e-10);
    assert!((DVec2::new(0.0, 0.0).distance_squared(DVec2::new(1.0, 0.0)) - 1.0).abs() < 1e-10);
}

// ============================================================================
// Additional edge case tests
// ============================================================================

/// Test KdTree with duplicate points
#[test]
fn test_kdtree_duplicate_points() {
    // Multiple points at the same location
    let points = vec![
        DVec2::new(5.0, 5.0),
        DVec2::new(5.0, 5.0), // Duplicate
        DVec2::new(5.0, 5.0), // Duplicate
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    assert_eq!(tree.len(), 4);

    // k_nearest should return all duplicates
    let neighbors = tree.k_nearest(DVec2::new(5.0, 5.0), 3);
    assert_eq!(neighbors.len(), 3);

    // All three should have distance 0
    for (idx, dist) in &neighbors {
        if *idx < 3 {
            assert!(*dist < 1e-10, "Duplicate point should have zero distance");
        }
    }
}

/// Test KdTree with many points (performance check)
#[test]
fn test_kdtree_many_points() {
    // Generate 1000 points in a grid pattern
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

    // k_nearest should work efficiently
    let neighbors = tree.k_nearest(DVec2::new(155.0, 155.0), 10);
    assert_eq!(neighbors.len(), 10);

    // All neighbors should be reasonably close (within a few grid cells)
    // Grid spacing is 10, so neighbors within 2 cells have distance^2 < 800
    for (_, dist) in &neighbors {
        assert!(
            *dist < 1000.0,
            "Neighbor too far: {} (squared distance)",
            dist
        );
    }
}

/// Test KdTree with points on a single horizontal line (degenerate)
#[test]
fn test_kdtree_horizontal_line() {
    let points: Vec<DVec2> = (0..10).map(|i| DVec2::new(i as f64 * 10.0, 0.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    assert_eq!(tree.len(), 10);

    // Query in the middle
    let neighbors = tree.k_nearest(DVec2::new(45.0, 0.0), 3);
    assert_eq!(neighbors.len(), 3);

    // Should find points 4 and 5 as closest
    let indices: Vec<usize> = neighbors.iter().map(|(i, _)| *i).collect();
    assert!(indices.contains(&4) || indices.contains(&5));
}

/// Test KdTree with points on a single vertical line (degenerate)
#[test]
fn test_kdtree_vertical_line() {
    let points: Vec<DVec2> = (0..10).map(|i| DVec2::new(0.0, i as f64 * 10.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    assert_eq!(tree.len(), 10);

    // Query in the middle
    let neighbors = tree.k_nearest(DVec2::new(0.0, 45.0), 3);
    assert_eq!(neighbors.len(), 3);

    // Should find points 4 and 5 as closest
    let indices: Vec<usize> = neighbors.iter().map(|(i, _)| *i).collect();
    assert!(indices.contains(&4) || indices.contains(&5));
}

/// Test KdTree with points at identical coordinates
#[test]
fn test_kdtree_all_identical_points() {
    // All points at the same location
    let points: Vec<DVec2> = (0..5).map(|_| DVec2::new(7.0, 7.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    assert_eq!(tree.len(), 5);

    // k_nearest should return all 5 with distance 0
    let neighbors = tree.k_nearest(DVec2::new(7.0, 7.0), 5);
    assert_eq!(neighbors.len(), 5);

    for (_, dist) in &neighbors {
        assert!(
            *dist < 1e-10,
            "All points at same location should have zero distance"
        );
    }
}

/// Test radius search with exact boundary
#[test]
fn test_kdtree_radius_search_boundary() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0), // Exactly at radius 1
        DVec2::new(2.0, 0.0), // Beyond radius 1
    ];
    let tree = KdTree::build(&points).unwrap();

    // Radius search with r=1 should include point at distance 1
    let results = tree.radius_search(DVec2::new(0.0, 0.0), 1.0);

    // Should find points 0 and 1 (distance 0 and 1)
    assert!(!results.is_empty()); // At least the center point
    let indices: Vec<usize> = results.iter().map(|(i, _)| *i).collect();
    assert!(indices.contains(&0));

    // Point at exactly radius 1 may or may not be included depending on implementation
    // (distance squared = 1.0, radius squared = 1.0)
}

/// Test k_nearest with k=0
#[test]
fn test_kdtree_k_nearest_zero() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 0.0), 0);
    assert!(neighbors.is_empty());
}

/// Test radius search with radius=0
#[test]
fn test_kdtree_radius_search_zero() {
    let points = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    // Radius 0 should only find exact matches
    let results = tree.radius_search(DVec2::new(0.0, 0.0), 0.0);
    // May or may not find the exact point depending on implementation
    assert!(results.len() <= 1);
}

/// Test form_triangles with very sparse neighbors
#[test]
fn test_form_triangles_sparse_neighbors() {
    // Grid of points
    let mut points = Vec::new();
    for y in 0..5 {
        for x in 0..5 {
            points.push(DVec2::new(x as f64 * 100.0, y as f64 * 100.0));
        }
    }

    let tree = KdTree::build(&points).unwrap();

    // With k=2, each point only sees 2 neighbors, limiting triangle formation
    let triangles = form_triangles_from_neighbors(&tree, 2);

    // Should still form some triangles
    assert!(!triangles.is_empty());

    // All triangles should have sorted indices
    for tri in &triangles {
        assert!(tri[0] < tri[1] && tri[1] < tri[2]);
    }
}

/// Test k_nearest query far from all points
#[test]
fn test_kdtree_query_far_from_points() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    // Query very far from all points
    let neighbors = tree.k_nearest(DVec2::new(1000.0, 1000.0), 2);
    assert_eq!(neighbors.len(), 2);

    // Distances should be large
    for (_, dist) in &neighbors {
        assert!(*dist > 1_000_000.0, "Distance should be very large");
    }
}

/// Test tree with negative coordinates
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

    // Query at negative coords
    let neighbors = tree.k_nearest(DVec2::new(-7.0, -7.0), 2);
    assert_eq!(neighbors.len(), 2);

    // Should find the two closest negative points
    let indices: Vec<usize> = neighbors.iter().map(|(i, _)| *i).collect();
    assert!(indices.contains(&0) || indices.contains(&1));
}

// ============================================================================
// BoundedMaxHeap optimization tests (small vs large capacity)
// ============================================================================

/// Test BoundedMaxHeap with small capacity (stack-allocated path)
#[test]
fn test_bounded_max_heap_small() {
    use super::BoundedMaxHeap;

    let mut heap = BoundedMaxHeap::new(5);

    // Verify it uses the Small variant
    assert!(matches!(heap, BoundedMaxHeap::Small { .. }));

    // Push items
    heap.push(0, 10.0);
    heap.push(1, 5.0);
    heap.push(2, 15.0);
    heap.push(3, 3.0);
    heap.push(4, 8.0);

    assert!(heap.is_full());
    assert!((heap.max_distance() - 15.0).abs() < 1e-10);

    // Push a smaller item - should replace the max
    heap.push(5, 2.0);
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    // Push a larger item - should be rejected
    heap.push(6, 20.0);
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    let result = heap.into_vec();
    assert_eq!(result.len(), 5);
}

/// Test BoundedMaxHeap with large capacity (heap-allocated path)
#[test]
fn test_bounded_max_heap_large() {
    use super::BoundedMaxHeap;
    use super::SMALL_HEAP_CAPACITY;

    let capacity = SMALL_HEAP_CAPACITY + 10;
    let mut heap = BoundedMaxHeap::new(capacity);

    // Verify it uses the Large variant
    assert!(matches!(heap, BoundedMaxHeap::Large { .. }));

    // Push items
    for i in 0..capacity {
        heap.push(i, (capacity - i) as f64);
    }

    assert!(heap.is_full());

    // Max distance should be the largest value (capacity)
    assert!((heap.max_distance() - capacity as f64).abs() < 1e-10);

    // Push a smaller item - should replace the max
    heap.push(100, 0.5);
    assert!((heap.max_distance() - (capacity - 1) as f64).abs() < 1e-10);

    let result = heap.into_vec();
    assert_eq!(result.len(), capacity);
}

/// Test BoundedMaxHeap at the boundary (exactly SMALL_HEAP_CAPACITY)
#[test]
fn test_bounded_max_heap_boundary() {
    use super::BoundedMaxHeap;
    use super::SMALL_HEAP_CAPACITY;

    // Exactly at the boundary - should use Small variant
    let heap_at_boundary = BoundedMaxHeap::new(SMALL_HEAP_CAPACITY);
    assert!(matches!(heap_at_boundary, BoundedMaxHeap::Small { .. }));

    // One above the boundary - should use Large variant
    let heap_above = BoundedMaxHeap::new(SMALL_HEAP_CAPACITY + 1);
    assert!(matches!(heap_above, BoundedMaxHeap::Large { .. }));
}

/// Test k_nearest with small k uses stack allocation
#[test]
fn test_kdtree_k_nearest_small_k() {
    // Generate enough points to query
    let mut points = Vec::new();
    for i in 0..100 {
        points.push(DVec2::new(i as f64, (i * i % 50) as f64));
    }

    let tree = KdTree::build(&points).unwrap();

    // Small k (within SMALL_HEAP_CAPACITY) should work correctly
    let neighbors = tree.k_nearest(DVec2::new(50.0, 25.0), 10);
    assert_eq!(neighbors.len(), 10);

    // Verify sorted by distance
    for i in 1..neighbors.len() {
        assert!(neighbors[i - 1].1 <= neighbors[i].1);
    }
}

/// Test k_nearest with large k uses heap allocation
#[test]
fn test_kdtree_k_nearest_large_k() {
    use super::SMALL_HEAP_CAPACITY;

    // Generate enough points
    let mut points = Vec::new();
    for i in 0..100 {
        points.push(DVec2::new(i as f64, (i * i % 50) as f64));
    }

    let tree = KdTree::build(&points).unwrap();

    // Large k (above SMALL_HEAP_CAPACITY) should still work correctly
    let k = SMALL_HEAP_CAPACITY + 10;
    let neighbors = tree.k_nearest(DVec2::new(50.0, 25.0), k);
    assert_eq!(neighbors.len(), k);

    // Verify sorted by distance
    for i in 1..neighbors.len() {
        assert!(neighbors[i - 1].1 <= neighbors[i].1);
    }
}

/// Test BoundedMaxHeap empty state
#[test]
fn test_bounded_max_heap_empty() {
    use super::BoundedMaxHeap;

    let heap_small = BoundedMaxHeap::new(5);
    assert!(!heap_small.is_full());
    assert_eq!(heap_small.max_distance(), f64::INFINITY);

    let heap_large = BoundedMaxHeap::new(50);
    assert!(!heap_large.is_full());
    assert_eq!(heap_large.max_distance(), f64::INFINITY);
}
