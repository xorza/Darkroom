//! Spatial data structures for efficient star queries.
//!
//! This module provides a k-d tree implementation optimized for 2D star positions,
//! enabling efficient nearest-neighbor queries for triangle formation.

#[cfg(test)]
mod tests;

/// A 2D k-d tree for efficient spatial queries on star positions.
///
/// This implementation is optimized for astronomical image registration where:
/// - Points are 2D (x, y) coordinates
/// - We need k-nearest-neighbor queries for triangle formation
/// - The tree is built once and queried many times
#[derive(Debug)]
pub struct KdTree {
    nodes: Vec<KdNode>,
    points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
struct KdNode {
    /// Index into the points array
    point_idx: usize,
    /// Left child index (None if leaf)
    left: Option<usize>,
    /// Right child index (None if leaf)
    right: Option<usize>,
    /// Split dimension (0 = x, 1 = y)
    split_dim: usize,
}

impl KdTree {
    /// Build a k-d tree from a list of points.
    ///
    /// The tree is built using the median-split strategy for balanced trees.
    ///
    /// # Arguments
    /// * `points` - List of (x, y) coordinates
    ///
    /// # Returns
    /// A new k-d tree, or None if points is empty
    pub fn build(points: &[(f64, f64)]) -> Option<Self> {
        if points.is_empty() {
            return None;
        }

        let points_vec: Vec<(f64, f64)> = points.to_vec();
        let mut indices: Vec<usize> = (0..points.len()).collect();
        let mut nodes = Vec::with_capacity(points.len());

        Self::build_recursive(&points_vec, &mut indices, 0, &mut nodes);

        Some(Self {
            nodes,
            points: points_vec,
        })
    }

    /// Recursively build the tree.
    fn build_recursive(
        points: &[(f64, f64)],
        indices: &mut [usize],
        depth: usize,
        nodes: &mut Vec<KdNode>,
    ) -> Option<usize> {
        if indices.is_empty() {
            return None;
        }

        let split_dim = depth % 2;

        // Sort indices by the split dimension
        indices.sort_by(|&a, &b| {
            let va = if split_dim == 0 {
                points[a].0
            } else {
                points[a].1
            };
            let vb = if split_dim == 0 {
                points[b].0
            } else {
                points[b].1
            };
            va.partial_cmp(&vb).unwrap()
        });

        let median = indices.len() / 2;
        let point_idx = indices[median];

        let node_idx = nodes.len();
        nodes.push(KdNode {
            point_idx,
            left: None,
            right: None,
            split_dim,
        });

        // Build left and right subtrees
        let (left_indices, right_part) = indices.split_at_mut(median);
        let right_indices = &mut right_part[1..]; // Skip the median

        let left = Self::build_recursive(points, left_indices, depth + 1, nodes);
        let right = Self::build_recursive(points, right_indices, depth + 1, nodes);

        nodes[node_idx].left = left;
        nodes[node_idx].right = right;

        Some(node_idx)
    }

    /// Find the k nearest neighbors to a query point.
    ///
    /// # Arguments
    /// * `query` - The query point (x, y)
    /// * `k` - Number of neighbors to find
    ///
    /// # Returns
    /// Vector of (index, distance_squared) pairs, sorted by distance
    pub fn k_nearest(&self, query: (f64, f64), k: usize) -> Vec<(usize, f64)> {
        if self.nodes.is_empty() || k == 0 {
            return Vec::new();
        }

        let mut heap = BoundedMaxHeap::new(k);
        self.k_nearest_recursive(0, query, &mut heap);

        let mut result: Vec<(usize, f64)> = heap.into_vec();
        result.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        result
    }

    /// Recursive k-nearest neighbor search.
    fn k_nearest_recursive(&self, node_idx: usize, query: (f64, f64), heap: &mut BoundedMaxHeap) {
        let node = &self.nodes[node_idx];
        let point = self.points[node.point_idx];

        // Calculate distance to current point
        let dist_sq = distance_squared(query, point);
        heap.push(node.point_idx, dist_sq);

        // Determine which subtree to search first
        let split_dim = node.split_dim;
        let query_val = if split_dim == 0 { query.0 } else { query.1 };
        let point_val = if split_dim == 0 { point.0 } else { point.1 };
        let diff = query_val - point_val;

        let (first, second) = if diff < 0.0 {
            (node.left, node.right)
        } else {
            (node.right, node.left)
        };

        // Search the nearer subtree first
        if let Some(first_idx) = first {
            self.k_nearest_recursive(first_idx, query, heap);
        }

        // Only search the other subtree if it could contain closer points
        let diff_sq = diff * diff;
        if let Some(second_idx) = second
            && (!heap.is_full() || diff_sq < heap.max_distance())
        {
            self.k_nearest_recursive(second_idx, query, heap);
        }
    }

    /// Find all points within a given radius.
    ///
    /// # Arguments
    /// * `query` - The query point (x, y)
    /// * `radius` - Search radius
    ///
    /// # Returns
    /// Vector of (index, distance_squared) pairs for all points within radius
    pub fn radius_search(&self, query: (f64, f64), radius: f64) -> Vec<(usize, f64)> {
        if self.nodes.is_empty() {
            return Vec::new();
        }

        let radius_sq = radius * radius;
        let mut results = Vec::new();
        self.radius_search_recursive(0, query, radius_sq, &mut results);
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        results
    }

    /// Recursive radius search.
    fn radius_search_recursive(
        &self,
        node_idx: usize,
        query: (f64, f64),
        radius_sq: f64,
        results: &mut Vec<(usize, f64)>,
    ) {
        let node = &self.nodes[node_idx];
        let point = self.points[node.point_idx];

        // Check if current point is within radius
        let dist_sq = distance_squared(query, point);
        if dist_sq <= radius_sq {
            results.push((node.point_idx, dist_sq));
        }

        // Determine which subtrees to search
        let split_dim = node.split_dim;
        let query_val = if split_dim == 0 { query.0 } else { query.1 };
        let point_val = if split_dim == 0 { point.0 } else { point.1 };
        let diff = query_val - point_val;
        let diff_sq = diff * diff;

        // Search subtrees that could contain points within radius
        if let Some(left_idx) = node.left
            && (diff <= 0.0 || diff_sq <= radius_sq)
        {
            self.radius_search_recursive(left_idx, query, radius_sq, results);
        }

        if let Some(right_idx) = node.right
            && (diff >= 0.0 || diff_sq <= radius_sq)
        {
            self.radius_search_recursive(right_idx, query, radius_sq, results);
        }
    }

    /// Get the number of points in the tree.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Get a point by index.
    pub fn get_point(&self, idx: usize) -> (f64, f64) {
        self.points[idx]
    }
}

/// Calculate squared Euclidean distance between two points.
#[inline]
fn distance_squared(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

/// A bounded max-heap for k-nearest neighbor search.
///
/// Keeps track of the k smallest distances seen so far.
#[derive(Debug)]
struct BoundedMaxHeap {
    capacity: usize,
    items: Vec<(usize, f64)>, // (index, distance_squared)
}

impl BoundedMaxHeap {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            items: Vec::with_capacity(capacity + 1),
        }
    }

    fn push(&mut self, idx: usize, dist_sq: f64) {
        if self.items.len() < self.capacity {
            self.items.push((idx, dist_sq));
            self.sift_up(self.items.len() - 1);
        } else if dist_sq < self.items[0].1 {
            // Replace the maximum (root) with new item
            self.items[0] = (idx, dist_sq);
            self.sift_down(0);
        }
    }

    fn is_full(&self) -> bool {
        self.items.len() >= self.capacity
    }

    fn max_distance(&self) -> f64 {
        if self.items.is_empty() {
            f64::INFINITY
        } else {
            self.items[0].1
        }
    }

    fn into_vec(self) -> Vec<(usize, f64)> {
        self.items
    }

    fn sift_up(&mut self, mut idx: usize) {
        while idx > 0 {
            let parent = (idx - 1) / 2;
            if self.items[idx].1 > self.items[parent].1 {
                self.items.swap(idx, parent);
                idx = parent;
            } else {
                break;
            }
        }
    }

    fn sift_down(&mut self, mut idx: usize) {
        loop {
            let left = 2 * idx + 1;
            let right = 2 * idx + 2;
            let mut largest = idx;

            if left < self.items.len() && self.items[left].1 > self.items[largest].1 {
                largest = left;
            }
            if right < self.items.len() && self.items[right].1 > self.items[largest].1 {
                largest = right;
            }

            if largest != idx {
                self.items.swap(idx, largest);
                idx = largest;
            } else {
                break;
            }
        }
    }
}

/// Form triangles using k-nearest neighbors from a k-d tree.
///
/// This is much more efficient than the brute-force O(n³) approach,
/// reducing complexity to approximately O(n * k²) where k is the
/// number of neighbors considered for each star.
///
/// # Arguments
/// * `tree` - K-d tree of star positions
/// * `k` - Number of nearest neighbors to consider for each star
///
/// # Returns
/// Vector of triangle vertex indices [i, j, k] where i < j < k
pub fn form_triangles_from_neighbors(tree: &KdTree, k: usize) -> Vec<[usize; 3]> {
    use std::collections::HashSet;

    let n = tree.len();
    if n < 3 {
        return Vec::new();
    }

    let k = k.min(n - 1);
    let mut triangles = HashSet::new();

    for i in 0..n {
        let point_i = tree.get_point(i);
        let neighbors = tree.k_nearest(point_i, k + 1); // +1 because point itself is included

        // Form triangles from pairs of neighbors
        for (ni, &(j, _)) in neighbors.iter().enumerate() {
            if j == i {
                continue;
            }
            for &(m, _) in neighbors.iter().skip(ni + 1) {
                if m == i {
                    continue;
                }

                // Normalize triangle indices to avoid duplicates
                let mut tri = [i, j, m];
                tri.sort();
                triangles.insert(tri);
            }
        }
    }

    triangles.into_iter().collect()
}
