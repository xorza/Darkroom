//! Spatial data structures for efficient star queries.
//!
//! This module provides a k-d tree implementation optimized for 2D star positions,
//! enabling efficient nearest-neighbor queries for triangle formation.

use glam::DVec2;

#[cfg(test)]
mod tests;

/// A nearest-neighbor result: the original point index and squared distance to the query.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Neighbor {
    pub index: usize,
    pub dist_sq: f64,
}

/// A work item for iterative k-d tree construction: the range [start, end) and current depth.
#[derive(Debug, Clone, Copy)]
struct BuildRange {
    start: usize,
    end: usize,
    depth: usize,
}

/// A 2D k-d tree for efficient spatial queries on star positions.
///
/// Uses a flat array layout where the tree structure is implicit in the
/// permuted index array. Each level alternates split dimension (x, y).
/// The median element of each range is the node; left/right children
/// occupy the sub-ranges before/after the median.
///
/// This layout eliminates per-node child pointers, improves cache locality,
/// and enables iterative construction.
#[derive(Debug)]
pub(crate) struct KdTree {
    /// Permuted point indices forming the implicit tree structure.
    /// For a range [start, end), the node is at index `mid = (start + end) / 2`.
    /// Left subtree is [start, mid), right subtree is [mid+1, end).
    indices: Vec<usize>,
    points: Vec<DVec2>,
}

impl KdTree {
    /// Build a k-d tree from a list of points.
    ///
    /// Uses iterative median-split construction with `select_nth_unstable`
    /// for O(n log n) partitioning without full sorting.
    ///
    /// # Arguments
    /// * `points` - List of point coordinates
    ///
    /// # Returns
    /// A new k-d tree, or None if points is empty
    pub fn build(points: &[DVec2]) -> Option<Self> {
        if points.is_empty() {
            return None;
        }

        let points_vec: Vec<DVec2> = points.to_vec();
        let mut indices: Vec<usize> = (0..points.len()).collect();

        // Iterative construction using an explicit work stack.
        let mut stack: Vec<BuildRange> = Vec::new();
        stack.push(BuildRange {
            start: 0,
            end: indices.len(),
            depth: 0,
        });

        while let Some(range) = stack.pop() {
            let len = range.end - range.start;
            if len <= 1 {
                continue;
            }

            let split_dim = range.depth % 2;
            let median = len / 2;

            // Partition around the median — O(n) per level instead of O(n log n) sort.
            indices[range.start..range.end].select_nth_unstable_by(median, |&a, &b| {
                let va = if split_dim == 0 {
                    points_vec[a].x
                } else {
                    points_vec[a].y
                };
                let vb = if split_dim == 0 {
                    points_vec[b].x
                } else {
                    points_vec[b].y
                };
                va.partial_cmp(&vb).unwrap()
            });

            let mid = range.start + median;
            // Push right first so left is processed first (stack is LIFO)
            if mid + 1 < range.end {
                stack.push(BuildRange {
                    start: mid + 1,
                    end: range.end,
                    depth: range.depth + 1,
                });
            }
            if range.start < mid {
                stack.push(BuildRange {
                    start: range.start,
                    end: mid,
                    depth: range.depth + 1,
                });
            }
        }

        Some(Self {
            indices,
            points: points_vec,
        })
    }

    /// Find the k nearest neighbors to a query point.
    ///
    /// # Arguments
    /// * `query` - The query point
    /// * `k` - Number of neighbors to find
    ///
    /// # Returns
    /// Vector of `Neighbor` results sorted by distance
    pub fn k_nearest(&self, query: DVec2, k: usize) -> Vec<Neighbor> {
        if self.indices.is_empty() || k == 0 {
            return Vec::new();
        }

        let mut heap = BoundedMaxHeap::new(k);
        self.k_nearest_range(0, self.indices.len(), 0, query, &mut heap);

        let mut result: Vec<Neighbor> = heap.into_vec();
        result.sort_by(|a, b| a.dist_sq.partial_cmp(&b.dist_sq).unwrap());
        result
    }

    /// K-nearest neighbor search over a range of the implicit tree.
    fn k_nearest_range(
        &self,
        start: usize,
        end: usize,
        depth: usize,
        query: DVec2,
        heap: &mut BoundedMaxHeap,
    ) {
        if start >= end {
            return;
        }

        let mid = start + (end - start) / 2;
        let point_idx = self.indices[mid];
        let point = self.points[point_idx];

        let dist_sq = (query - point).length_squared();
        heap.push(Neighbor {
            index: point_idx,
            dist_sq,
        });

        let split_dim = depth % 2;
        let query_val = if split_dim == 0 { query.x } else { query.y };
        let point_val = if split_dim == 0 { point.x } else { point.y };
        let diff = query_val - point_val;

        // Search the nearer subtree first
        let (first_start, first_end, second_start, second_end) = if diff < 0.0 {
            (start, mid, mid + 1, end)
        } else {
            (mid + 1, end, start, mid)
        };

        self.k_nearest_range(first_start, first_end, depth + 1, query, heap);

        // Only search the other subtree if it could contain closer points
        let diff_sq = diff * diff;
        if !heap.is_full() || diff_sq < heap.max_distance() {
            self.k_nearest_range(second_start, second_end, depth + 1, query, heap);
        }
    }

    /// Find all point indices within a given radius, appending to a buffer.
    ///
    /// The buffer is cleared before use. This avoids allocations when
    /// called repeatedly in a loop.
    pub fn radius_indices_into(&self, query: DVec2, radius: f64, indices: &mut Vec<usize>) {
        indices.clear();
        if self.indices.is_empty() {
            return;
        }
        let radius_sq = radius * radius;
        self.radius_indices_range(0, self.indices.len(), 0, query, radius_sq, indices);
    }

    /// Radius search collecting only indices over a range of the implicit tree.
    fn radius_indices_range(
        &self,
        start: usize,
        end: usize,
        depth: usize,
        query: DVec2,
        radius_sq: f64,
        results: &mut Vec<usize>,
    ) {
        if start >= end {
            return;
        }

        let mid = start + (end - start) / 2;
        let point_idx = self.indices[mid];
        let point = self.points[point_idx];

        let dist_sq = (query - point).length_squared();
        if dist_sq <= radius_sq {
            results.push(point_idx);
        }

        let split_dim = depth % 2;
        let query_val = if split_dim == 0 { query.x } else { query.y };
        let point_val = if split_dim == 0 { point.x } else { point.y };
        let diff = query_val - point_val;
        let diff_sq = diff * diff;

        if diff <= 0.0 || diff_sq <= radius_sq {
            self.radius_indices_range(start, mid, depth + 1, query, radius_sq, results);
        }

        if diff >= 0.0 || diff_sq <= radius_sq {
            self.radius_indices_range(mid + 1, end, depth + 1, query, radius_sq, results);
        }
    }

    /// Get the number of points in the tree.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Get a point by index.
    pub fn get_point(&self, idx: usize) -> DVec2 {
        self.points[idx]
    }
}

/// Maximum capacity for stack-allocated neighbor collection.
/// For k <= this value, we avoid heap allocation entirely.
const SMALL_HEAP_CAPACITY: usize = 32;

/// A bounded max-heap for k-nearest neighbor search.
///
/// Keeps track of the k smallest distances seen so far.
/// Uses stack allocation for small k (<=32), heap allocation for larger k.
///
/// The large size difference between variants is intentional - the Small variant
/// avoids heap allocation for the common case of k <= 32.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum BoundedMaxHeap {
    /// Stack-allocated variant for small k values
    Small {
        capacity: usize,
        len: usize,
        items: [Neighbor; SMALL_HEAP_CAPACITY],
    },
    /// Heap-allocated variant for larger k values
    Large {
        capacity: usize,
        items: Vec<Neighbor>,
    },
}

impl BoundedMaxHeap {
    fn new(capacity: usize) -> Self {
        let zero = Neighbor {
            index: 0,
            dist_sq: 0.0,
        };
        if capacity <= SMALL_HEAP_CAPACITY {
            BoundedMaxHeap::Small {
                capacity,
                len: 0,
                items: [zero; SMALL_HEAP_CAPACITY],
            }
        } else {
            BoundedMaxHeap::Large {
                capacity,
                items: Vec::with_capacity(capacity + 1),
            }
        }
    }

    fn push(&mut self, neighbor: Neighbor) {
        match self {
            BoundedMaxHeap::Small {
                capacity,
                len,
                items,
            } => {
                if *len < *capacity {
                    items[*len] = neighbor;
                    *len += 1;
                    Self::sift_up_slice(&mut items[..*len], *len - 1);
                } else if neighbor.dist_sq < items[0].dist_sq {
                    items[0] = neighbor;
                    Self::sift_down_slice(&mut items[..*len], 0);
                }
            }
            BoundedMaxHeap::Large { capacity, items } => {
                if items.len() < *capacity {
                    items.push(neighbor);
                    let last_idx = items.len() - 1;
                    Self::sift_up_slice(items, last_idx);
                } else if neighbor.dist_sq < items[0].dist_sq {
                    items[0] = neighbor;
                    Self::sift_down_slice(items, 0);
                }
            }
        }
    }

    fn is_full(&self) -> bool {
        match self {
            BoundedMaxHeap::Small { capacity, len, .. } => *len >= *capacity,
            BoundedMaxHeap::Large { capacity, items } => items.len() >= *capacity,
        }
    }

    fn max_distance(&self) -> f64 {
        match self {
            BoundedMaxHeap::Small { len, items, .. } => {
                if *len == 0 {
                    f64::INFINITY
                } else {
                    items[0].dist_sq
                }
            }
            BoundedMaxHeap::Large { items, .. } => {
                if items.is_empty() {
                    f64::INFINITY
                } else {
                    items[0].dist_sq
                }
            }
        }
    }

    fn into_vec(self) -> Vec<Neighbor> {
        match self {
            BoundedMaxHeap::Small { len, items, .. } => items[..len].to_vec(),
            BoundedMaxHeap::Large { items, .. } => items,
        }
    }

    fn sift_up_slice(items: &mut [Neighbor], mut idx: usize) {
        while idx > 0 {
            let parent = (idx - 1) / 2;
            if items[idx].dist_sq > items[parent].dist_sq {
                items.swap(idx, parent);
                idx = parent;
            } else {
                break;
            }
        }
    }

    fn sift_down_slice(items: &mut [Neighbor], mut idx: usize) {
        let len = items.len();
        loop {
            let left = 2 * idx + 1;
            let right = 2 * idx + 2;
            let mut largest = idx;

            if left < len && items[left].dist_sq > items[largest].dist_sq {
                largest = left;
            }
            if right < len && items[right].dist_sq > items[largest].dist_sq {
                largest = right;
            }

            if largest != idx {
                items.swap(idx, largest);
                idx = largest;
            } else {
                break;
            }
        }
    }
}
