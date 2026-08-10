//! Spatial data structures for efficient star queries.
//!
//! This module provides a k-d tree implementation optimized for 2D star positions,
//! enabling efficient nearest-neighbor queries for triangle formation.

use glam::DVec2;
use smallvec::SmallVec;

/// Extract the coordinate for the given split dimension (0 = x, 1 = y).
#[inline(always)]
fn dim_value(p: DVec2, dim: usize) -> f64 {
    if dim == 0 { p.x } else { p.y }
}

/// A nearest-neighbor result: the original point index and squared distance to the query.
#[derive(Debug, Clone, Copy)]
pub(super) struct Neighbor {
    pub(super) index: usize,
    pub(super) dist_sq: f64,
}

/// One subtree of the implicit layout: the `indices` range `[start, end)` it occupies, and the
/// depth it hangs at — which is what fixes its split dimension.
///
/// Carried as a unit by both walks over the tree, the iterative build and the recursive descent,
/// so neither passes three bare `usize`s that nothing but their order distinguishes.
#[derive(Debug, Clone, Copy)]
struct Subtree {
    start: usize,
    end: usize,
    depth: usize,
}

impl Subtree {
    /// The whole tree, at depth zero.
    fn root(len: usize) -> Self {
        Self {
            start: 0,
            end: len,
            depth: 0,
        }
    }

    fn len(self) -> usize {
        self.end - self.start
    }

    /// The node index this subtree splits on: the median of its range.
    fn mid(self) -> usize {
        self.start + self.len() / 2
    }

    /// Levels alternate x and y.
    fn split_dim(self) -> usize {
        self.depth % 2
    }

    /// The two halves either side of [`Self::mid`], one level down.
    fn children(self) -> [Self; 2] {
        let (mid, depth) = (self.mid(), self.depth + 1);
        [
            Self {
                start: self.start,
                end: mid,
                depth,
            },
            Self {
                start: mid + 1,
                end: self.end,
                depth,
            },
        ]
    }
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
pub(super) struct KdTree {
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
    pub(super) fn build(points: &[DVec2]) -> Option<Self> {
        if points.is_empty() {
            return None;
        }

        let points_vec: Vec<DVec2> = points.to_vec();
        let mut indices: Vec<usize> = (0..points.len()).collect();

        // Iterative construction using an explicit work stack.
        let mut stack: Vec<Subtree> = vec![Subtree::root(indices.len())];

        while let Some(subtree) = stack.pop() {
            if subtree.len() <= 1 {
                continue;
            }

            let split_dim = subtree.split_dim();
            // Partition around the median — O(n) per level instead of O(n log n) sort.
            indices[subtree.start..subtree.end].select_nth_unstable_by(
                subtree.len() / 2,
                |&a, &b| {
                    dim_value(points_vec[a], split_dim)
                        .total_cmp(&dim_value(points_vec[b], split_dim))
                },
            );

            // Push right first so left is processed first (stack is LIFO).
            let [left, right] = subtree.children();
            if right.len() > 0 {
                stack.push(right);
            }
            if left.len() > 0 {
                stack.push(left);
            }
        }

        Some(Self {
            indices,
            points: points_vec,
        })
    }

    /// Find the `k` nearest neighbors to `query`, filling `out` (cleared first) instead of
    /// allocating — for hot loops that query repeatedly (e.g. the per-star k-NN graph).
    /// Results are sorted by ascending distance.
    pub(super) fn k_nearest_into(&self, query: DVec2, k: usize, out: &mut Vec<Neighbor>) {
        out.clear();
        if k == 0 || self.indices.is_empty() {
            return;
        }

        let mut heap = BoundedMaxHeap::new(k);
        self.descend(Subtree::root(self.indices.len()), query, &mut heap);

        heap.write_into(out);
        out.sort_by(|a, b| a.dist_sq.total_cmp(&b.dist_sq));
    }

    /// Find the single nearest neighbor to a query point.
    ///
    /// More efficient than `k_nearest(query, 1)` — uses a scalar best-distance
    /// tracker with no heap or Vec allocation.
    pub(super) fn nearest_one(&self, query: DVec2) -> Option<Neighbor> {
        if self.indices.is_empty() {
            return None;
        }
        let mut best = NearestOne(Neighbor {
            index: 0,
            dist_sq: f64::INFINITY,
        });
        self.descend(Subtree::root(self.indices.len()), query, &mut best);
        best.0.dist_sq.is_finite().then_some(best.0)
    }

    /// Find all point indices within a given radius, appending to a buffer.
    ///
    /// The buffer is cleared before use. This avoids allocations when
    /// called repeatedly in a loop. The results are in traversal order, not sorted.
    pub(super) fn radius_indices_into(&self, query: DVec2, radius: f64, indices: &mut Vec<usize>) {
        indices.clear();
        if self.indices.is_empty() {
            return;
        }
        let mut within = WithinRadius {
            radius_sq: radius * radius,
            indices,
        };
        self.descend(Subtree::root(self.indices.len()), query, &mut within);
    }

    /// Offer `visitor` every point under `subtree` it cannot rule out: visit the median, recurse
    /// into the side of the split `query` falls on, then skip the other side when the split plane
    /// is already further off than anything the visitor would still accept. Start from
    /// [`Subtree::root`].
    ///
    /// Which points are kept, and how far "still accept" reaches, are the visitor's business — the
    /// descent knows neither, which is what lets one traversal serve k-nearest, nearest-one and
    /// radius search.
    fn descend(&self, subtree: Subtree, query: DVec2, visitor: &mut impl Descent) {
        if subtree.len() == 0 {
            return;
        }

        let point_idx = self.indices[subtree.mid()];
        let point = self.points[point_idx];
        visitor.visit(point_idx, (query - point).length_squared());

        let split_dim = subtree.split_dim();
        let diff = dim_value(query, split_dim) - dim_value(point, split_dim);

        // Nearer side first, so the visitor's bound tightens before the far side is weighed.
        let [left, right] = subtree.children();
        let (near, far) = if diff < 0.0 {
            (left, right)
        } else {
            (right, left)
        };

        self.descend(near, query, visitor);

        // Inclusive: a radius search must keep points sitting exactly on its boundary, and for the
        // two nearest-neighbour visitors an equal-distance far side is only ever a wasted visit —
        // neither replaces a held neighbour on a tie.
        if diff * diff <= visitor.prune_radius_sq() {
            self.descend(far, query, visitor);
        }
    }

    /// Get the number of points in the tree.
    pub(super) fn len(&self) -> usize {
        self.points.len()
    }

    /// Get a point by index.
    pub(super) fn get_point(&self, idx: usize) -> DVec2 {
        self.points[idx]
    }
}

/// What a [`KdTree::descend`] is collecting, and how far it still has to look to collect it.
///
/// The three queries differ only in these two answers, so they are what the descent takes instead
/// of being written into three copies of it.
trait Descent {
    /// Offer the point at `index`, `dist_sq` away from the query. Keeping it is the visitor's call.
    fn visit(&mut self, index: usize, dist_sq: f64);

    /// Squared distance past which no point can still be wanted, so a subtree whose split plane is
    /// at least that far away need not be entered. `INFINITY` while nothing bounds the search yet.
    fn prune_radius_sq(&self) -> f64;
}

impl Descent for BoundedMaxHeap {
    fn visit(&mut self, index: usize, dist_sq: f64) {
        self.push(Neighbor { index, dist_sq });
    }

    fn prune_radius_sq(&self) -> f64 {
        // Until `k` neighbours are in hand every subtree can still contribute, however far off it
        // is — the heap's root is only an upper bound once it is full.
        if self.is_full() {
            self.max_distance()
        } else {
            f64::INFINITY
        }
    }
}

/// The best neighbour seen so far. `dist_sq` starts at infinity, which reads as "nothing found".
#[derive(Debug)]
struct NearestOne(Neighbor);

impl Descent for NearestOne {
    fn visit(&mut self, index: usize, dist_sq: f64) {
        if dist_sq < self.0.dist_sq {
            self.0 = Neighbor { index, dist_sq };
        }
    }

    fn prune_radius_sq(&self) -> f64 {
        self.0.dist_sq
    }
}

/// Every point inside a fixed radius. Unlike the two nearest-neighbour visitors its bound never
/// tightens, so the descent prunes against the same figure the whole way down.
#[derive(Debug)]
struct WithinRadius<'a> {
    radius_sq: f64,
    indices: &'a mut Vec<usize>,
}

impl Descent for WithinRadius<'_> {
    fn visit(&mut self, index: usize, dist_sq: f64) {
        if dist_sq <= self.radius_sq {
            self.indices.push(index);
        }
    }

    fn prune_radius_sq(&self) -> f64 {
        self.radius_sq
    }
}

/// Neighbours a k-nearest query holds inline before it spills to the heap. `k` is the caller's,
/// and the tree is queried per star, so the common small-k case must not allocate.
const SMALL_HEAP_CAPACITY: usize = 32;

/// A bounded max-heap for k-nearest neighbor search: the `k` smallest distances seen so far, with
/// the largest of them at the root, so the one to evict is always in hand.
///
/// [`SmallVec`] rather than a `Vec`, so `k <= SMALL_HEAP_CAPACITY` stays on the stack and a larger
/// one spills — the storage split without a second copy of the heap operations to go with it.
#[derive(Debug)]
struct BoundedMaxHeap {
    /// The caller's `k`. Not `items.capacity()`, which is the inline size until the heap spills.
    capacity: usize,
    items: SmallVec<[Neighbor; SMALL_HEAP_CAPACITY]>,
}

impl BoundedMaxHeap {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            items: SmallVec::with_capacity(capacity),
        }
    }

    fn push(&mut self, neighbor: Neighbor) {
        if self.items.len() < self.capacity {
            self.items.push(neighbor);
            let last = self.items.len() - 1;
            Self::sift_up_slice(&mut self.items, last);
        } else if neighbor.dist_sq < self.items[0].dist_sq {
            self.items[0] = neighbor;
            Self::sift_down_slice(&mut self.items, 0);
        }
    }

    fn is_full(&self) -> bool {
        self.items.len() >= self.capacity
    }

    fn max_distance(&self) -> f64 {
        self.items
            .first()
            .map_or(f64::INFINITY, |neighbor| neighbor.dist_sq)
    }

    /// Append the heap's contents to `out` (unsorted) without consuming the heap.
    fn write_into(&self, out: &mut Vec<Neighbor>) {
        out.extend_from_slice(&self.items);
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

/// Test-only ergonomic wrapper: k-nearest as an owned, distance-sorted `Vec`. Production uses
/// the buffer-reusing [`KdTree::k_nearest_into`]; this allocating form exists only for test
/// readability, so it's gated out of the library build.
#[cfg(test)]
mod internals {
    use super::*;

    impl KdTree {
        pub(super) fn k_nearest(&self, query: DVec2, k: usize) -> Vec<Neighbor> {
            let mut out = Vec::new();
            self.k_nearest_into(query, k, &mut out);
            out
        }
    }
}

#[cfg(test)]
mod tests;
