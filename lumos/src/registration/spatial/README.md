# Spatial Module

2D k-d tree for efficient spatial queries on star positions. Used for triangle formation (k-nearest-neighbor), invariant-space lookup (radius search), and guided matching in the registration pipeline.

## Structure

- `mod.rs` — `KdTree`, `BoundedMaxHeap`, `form_triangles_from_neighbors()`
- `tests.rs` — Unit tests for all public and internal APIs

## Usage

`KdTree` is used in three places:

1. **Triangle matching** (`triangle/matching.rs`) — Build tree from star positions, query k-nearest neighbors to form triangles via `form_triangles_from_neighbors()`.
2. **Triangle voting** (`triangle/voting.rs`) — Build tree on triangle invariant ratios, query by radius to find similar triangles.
3. **Pipeline guided matching** (`pipeline/mod.rs`) — Build tree from target stars, query nearest neighbor to find additional matches after RANSAC.
4. **Astrometry quad hashing** (`astrometry/quad_hash.rs`) — Build tree from star positions for quad formation.

## API

- `KdTree::build(points) -> Option<Self>` — Median-split construction, O(n log n).
- `k_nearest(query, k) -> Vec<(usize, f64)>` — K-nearest neighbors, sorted by distance squared.
- `radius_indices_into(query, radius, &mut buf)` — All indices within radius, zero-allocation reuse.
- `radius_search(query, radius)` — All (index, dist_sq) within radius (test-only).
- `form_triangles_from_neighbors(tree, k)` — Form unique triangles from k-nearest neighbor pairs.

## Naming: `spatial` vs `kdtree`

The current name `spatial` is appropriate. The module contains both `KdTree` and `form_triangles_from_neighbors()`, which is a spatial algorithm that uses the tree but isn't part of the tree itself. If the module only contained the k-d tree struct, `kdtree` would be better. Since it bundles spatial algorithms alongside the data structure, `spatial` is the more accurate name. No rename needed.

## Comparison with External Crates

### kiddo (v5.x) — the dominant Rust k-d tree crate

Key differences from our implementation:

| Feature | Our KdTree | kiddo |
|---------|-----------|-------|
| Dimensions | Hardcoded 2D (DVec2) | Const-generic N-dimensional |
| Construction | Recursive median split | Balanced + cache-optimized layout |
| Immutable tree | No | Yes (`ImmutableKdTree`, faster queries) |
| Node layout | Vec of structs with Option child pointers | Flat array, Eytzinger/Van Emde Boas ordering |
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
- If immutable tree's cache-friendly layout measurably improves performance.

## Potential Improvements

### 1. Iterative construction instead of recursive (low priority)

Current recursive `build_recursive` could stack-overflow for extremely unbalanced inputs (pathological case). Not a real risk with 200 stars, but an iterative approach with an explicit stack would be more robust.

### 2. Replace `sort_by` with `select_nth_unstable` in build (easy win)

Current code fully sorts indices at each level to find the median. Using `select_nth_unstable_by` would reduce construction from O(n log^2 n) to O(n log n) by only partitioning around the median without fully sorting. This is a standard k-d tree optimization.

```rust
// Current: O(n log n) per level
indices.sort_by(|&a, &b| ...);

// Better: O(n) per level
indices.select_nth_unstable_by(median, |&a, &b| ...);
```

### 3. Flat/compact node representation (low priority)

Current nodes store `Option<usize>` for left/right children. A flat implicit tree (like a binary heap array) would eliminate these pointers and improve cache locality. However, this only matters for large trees. With 200 nodes, the entire tree fits in L1 cache regardless of layout.

### 4. Move `form_triangles_from_neighbors` to triangle module (consider)

This function is specific to triangle matching — it forms triangles from k-nearest neighbor pairs. It could live in `triangle/matching.rs` alongside `form_triangles_kdtree` which calls it. The spatial module would then be a pure data structure. Counter-argument: it's a spatial algorithm that operates directly on `KdTree`, so it fits here too.

### 5. Remove `#[cfg(test)]` from `radius_search` (if needed)

`radius_search` returns `(index, distance_squared)` pairs but is test-only. `radius_indices_into` is the production version (indices only, buffer reuse). If distance information is ever needed in production, promote `radius_search`. Otherwise, the current split is clean.
