//! Tests for the spatial module (k-d tree).

use super::*;

#[test]
fn test_kdtree_build_empty() {
    let tree = KdTree::build(&[]);
    assert!(tree.is_none());
}

#[test]
fn test_kdtree_build_single_point() {
    let points = vec![(1.0, 2.0)];
    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 1);
}

#[test]
fn test_kdtree_build_multiple_points() {
    let points = vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 4);
}

#[test]
fn test_kdtree_k_nearest_basic() {
    let points = vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)];
    let tree = KdTree::build(&points).unwrap();

    // Find 3 nearest to center point (0.5, 0.5)
    let neighbors = tree.k_nearest((0.5, 0.5), 3);
    assert_eq!(neighbors.len(), 3);

    // The center point should be closest to itself (index 4)
    // and then to the corners equidistantly
    assert_eq!(neighbors[0].0, 4); // Center point itself
    assert!(neighbors[0].1 < 0.001); // Distance should be ~0
}

#[test]
fn test_kdtree_k_nearest_finds_exact_point() {
    let points = vec![(0.0, 0.0), (10.0, 10.0), (5.0, 5.0)];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest((5.0, 5.0), 1);
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].0, 2);
    assert!(neighbors[0].1 < 1e-10);
}

#[test]
fn test_kdtree_k_nearest_order() {
    let points = vec![
        (0.0, 0.0),  // index 0, distance from (0,0) = 0
        (1.0, 0.0),  // index 1, distance from (0,0) = 1
        (2.0, 0.0),  // index 2, distance from (0,0) = 4
        (3.0, 0.0),  // index 3, distance from (0,0) = 9
        (10.0, 0.0), // index 4, distance from (0,0) = 100
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest((0.0, 0.0), 3);
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
    let points = vec![(0.0, 0.0), (1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest((0.0, 0.0), 10);
    assert_eq!(neighbors.len(), 2); // Only 2 points available
}

#[test]
fn test_kdtree_radius_search_basic() {
    let points = vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (5.0, 5.0), (10.0, 10.0)];
    let tree = KdTree::build(&points).unwrap();

    // Search within radius 2 from origin
    let results = tree.radius_search((0.0, 0.0), 2.0);

    // Should find points 0, 1, 2 (all within distance 2)
    assert_eq!(results.len(), 3);
    let indices: Vec<usize> = results.iter().map(|(i, _)| *i).collect();
    assert!(indices.contains(&0));
    assert!(indices.contains(&1));
    assert!(indices.contains(&2));
}

#[test]
fn test_kdtree_radius_search_empty_result() {
    let points = vec![(0.0, 0.0), (10.0, 10.0)];
    let tree = KdTree::build(&points).unwrap();

    // Search in an area with no points
    let results = tree.radius_search((5.0, 5.0), 1.0);
    assert!(results.is_empty());
}

#[test]
fn test_kdtree_radius_search_all_points() {
    let points = vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    // Large radius should find all points
    let results = tree.radius_search((0.5, 0.5), 10.0);
    assert_eq!(results.len(), 4);
}

#[test]
fn test_form_triangles_from_neighbors_basic() {
    let points = vec![
        (0.0, 0.0),
        (1.0, 0.0),
        (0.5, 0.866), // Equilateral triangle
    ];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);

    // Should form exactly one triangle
    assert_eq!(triangles.len(), 1);
    assert_eq!(triangles[0], [0, 1, 2]);
}

#[test]
fn test_form_triangles_from_neighbors_square() {
    let points = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
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
    let points = vec![(0.0, 0.0), (1.0, 0.0)];
    let tree = KdTree::build(&points).unwrap();

    let triangles = form_triangles_from_neighbors(&tree, 3);
    assert!(triangles.is_empty());
}

#[test]
fn test_form_triangles_from_neighbors_no_duplicates() {
    let points = vec![
        (0.0, 0.0),
        (1.0, 0.0),
        (2.0, 0.0),
        (0.0, 1.0),
        (1.0, 1.0),
        (2.0, 1.0),
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
        (0.0, 0.0),
        (1.0, 0.0),
        (2.0, 0.0),
        (0.0, 1.0),
        (1.0, 1.0),
        (2.0, 1.0),
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
        (1024.5, 768.3),
        (2048.1, 1536.7),
        (512.9, 384.2),
        (3072.0, 2304.5),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest((1024.5, 768.3), 2);
    assert_eq!(neighbors.len(), 2);
    assert_eq!(neighbors[0].0, 0); // Exact match
}

#[test]
fn test_kdtree_with_collinear_points() {
    // Points on a line
    let points = vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0), (4.0, 4.0)];
    let tree = KdTree::build(&points).unwrap();

    // k-nearest should still work
    let neighbors = tree.k_nearest((2.0, 2.0), 3);
    assert_eq!(neighbors.len(), 3);
    assert_eq!(neighbors[0].0, 2); // Exact match
}

#[test]
fn test_kdtree_with_clustered_points() {
    // Two clusters of points
    let mut points = Vec::new();

    // Cluster 1 around (0, 0)
    for i in 0..5 {
        points.push((i as f64 * 0.1, i as f64 * 0.1));
    }

    // Cluster 2 around (100, 100)
    for i in 0..5 {
        points.push((100.0 + i as f64 * 0.1, 100.0 + i as f64 * 0.1));
    }

    let tree = KdTree::build(&points).unwrap();

    // Query near cluster 1 should return cluster 1 points
    let neighbors = tree.k_nearest((0.0, 0.0), 5);
    for (idx, _) in neighbors {
        assert!(idx < 5, "Should only find points from cluster 1");
    }

    // Query near cluster 2 should return cluster 2 points
    let neighbors = tree.k_nearest((100.0, 100.0), 5);
    for (idx, _) in neighbors {
        assert!(idx >= 5, "Should only find points from cluster 2");
    }
}

#[test]
fn test_distance_squared() {
    assert!((distance_squared((0.0, 0.0), (3.0, 4.0)) - 25.0).abs() < 1e-10);
    assert!((distance_squared((1.0, 1.0), (1.0, 1.0))).abs() < 1e-10);
    assert!((distance_squared((0.0, 0.0), (1.0, 0.0)) - 1.0).abs() < 1e-10);
}
