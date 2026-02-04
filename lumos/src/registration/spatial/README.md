# Spatial Module

2D k-d tree for efficient spatial queries on star positions. Used for k-nearest-neighbor queries, invariant-space radius lookup, and guided matching in the registration pipeline.

## Structure

- `mod.rs` — `KdTree`, `Neighbor`, `BoundedMaxHeap`
- `tests.rs` — Unit tests for all public and internal APIs

## Usage

`KdTree` is used in four places:

1. **Triangle matching** (`triangle/matching.rs`) — Build tree from star positions, query k-nearest neighbors to form triangles.
2. **Triangle voting** (`triangle/voting.rs`) — Build tree on triangle invariant ratios, query by radius to find similar triangles.
3. **Pipeline guided matching** (`pipeline/mod.rs`) — Build tree from target stars, query nearest neighbor to find additional matches after RANSAC.
4. **Astrometry quad hashing** (`astrometry/quad_hash.rs`) — Build tree from star positions for quad formation.

## API

- `KdTree::build(points) -> Option<Self>` — Iterative median-split construction, O(n log n).
- `k_nearest(query, k) -> Vec<Neighbor>` — K-nearest neighbors, sorted by distance squared. `Neighbor` has `index` and `dist_sq` fields.
- `radius_indices_into(query, radius, &mut buf)` — All indices within radius, zero-allocation reuse.
- `len()` — Number of points in the tree.
- `get_point(idx)` — Get a point by its original index.

## Implementation

### Flat implicit tree layout

The tree uses a flat array of permuted point indices. The tree structure is implicit in the array layout — for any range `[start, end)`, the node is at index `mid = start + (end - start) / 2`, the left subtree is `[start, mid)`, and the right subtree is `[mid+1, end)`.

This eliminates per-node child pointers (`Option<usize>` left/right), reducing memory from `~40 bytes/node` (old struct with point_idx, left, right, split_dim) to `8 bytes/node` (just the index). Split dimension is implicit from the depth.

### Iterative construction

Build uses an explicit work stack instead of recursion, avoiding stack overflow for any input size. Each level uses `select_nth_unstable_by` for O(n) median partitioning instead of O(n log n) full sorting.

### BoundedMaxHeap

K-nearest neighbor search uses a custom bounded max-heap that tracks the k smallest distances. Uses stack allocation for k <= 32 (the common case), falling back to heap allocation for larger k.

## Comparison with External Crates

### kiddo (v5.x) — the dominant Rust k-d tree crate

| Feature | Our KdTree | kiddo |
|---------|-----------|-------|
| Dimensions | Hardcoded 2D (DVec2) | Const-generic N-dimensional |
| Construction | Iterative median split | Balanced + cache-optimized layout |
| Node layout | Flat implicit array (8 bytes/node) | Flat array, Eytzinger/Van Emde Boas ordering |
| SIMD | No | Optional (nightly, f64 `nearest_one`) |
| Serialization | No | rkyv zero-copy support |
| Coordinate types | f64 only | f32, f64, f16, fixed-point |
| KNN heap | Custom `BoundedMaxHeap` (stack for k<=32) | Similar bounded heap |

### Verdict: Keep custom implementation

Reasons to keep:
- **Simplicity**: ~300 lines, easy to understand and modify. kiddo is a large dependency.
- **DVec2 integration**: Direct use of glam types avoids coordinate conversion overhead.
- **Sufficient performance**: With max 200 stars, the tree has at most 200 nodes. Construction and queries are microsecond-scale. kiddo's cache-line optimizations matter for millions of points, not hundreds.
- **No unused features**: We don't need serialization, SIMD, N-dimensional support, or mutable trees.

Reasons to consider kiddo in the future:
- If point counts grow significantly (>10,000 points).
- If the tree is used in a hot loop with millions of queries.
