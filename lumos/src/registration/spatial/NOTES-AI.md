# Spatial Module (K-D Tree) -- Research Notes

## Overview

A custom 2D k-d tree implementation in ~390 lines (`mod.rs`) for efficient spatial queries on star positions. Provides k-nearest-neighbor (KNN) and radius search. Used in four places in the registration pipeline:

1. **Triangle formation** (`triangle/matching.rs:20-23`) -- k-d tree on star positions, KNN queries to form triangles.
2. **Invariant-space matching** (`triangle/voting.rs:96-101`) -- k-d tree on triangle ratio pairs (2D), radius queries for similar triangles.
3. **Match recovery** (`mod.rs:354-357`) -- k-d tree on target stars, 1-NN queries to find additional inlier matches after RANSAC.

Dimensionality is always 2 (DVec2). Typical point counts: 50-200 stars for triangle formation; thousands of triangles for invariant-space lookup.

## Implementation Correctness

### Construction (lines 55-117)

**Layout**: Implicit flat array. The `indices` Vec stores a permutation of `[0..n)`. For any range `[start, end)`, the node is at `mid = start + (end - start) / 2`. Left subtree is `[start, mid)`, right subtree is `[mid+1, end)`. Split dimension alternates by depth: `depth % 2` (x=0, y=1).

**Median selection**: Uses `select_nth_unstable_by` (line 81) with `median = len / 2`. This is Rust's introselect algorithm, guaranteed O(n) worst-case since Rust 1.77 (PR #107522 added median-of-medians fallback). Prior versions had O(n^2) worst case for adversarial inputs, but this is now fixed.

**Correctness of index arithmetic**: Build computes `mid = range.start + median` where `median = len / 2` (line 78, 95). Search computes `mid = start + (end - start) / 2` (line 153). These are algebraically identical: `start + (end - start) / 2 = start + len / 2`. Consistent.

**Balance**: For even-length ranges, `len / 2` favors the left subtree. E.g., for 4 elements: left gets 2, right gets 1. This is standard and correct -- a perfectly balanced tree is only possible for `n = 2^k - 1`. The slight left bias does not affect correctness or asymptotic performance.

**Complexity**: O(n) work per level (one `select_nth_unstable_by` call partitions the current range), O(log n) levels. Total: O(n log n). Space: O(n) for the indices array plus O(n) for the cloned points.

**Edge cases handled**:
- Empty input returns `None` (line 56-58).
- Single-element ranges skipped (line 73-75: `len <= 1`).
- All identical points: `partial_cmp` returns `Equal` for ties. `select_nth_unstable_by` handles ties correctly (places them on either side). The tree degrades to linear but remains correct.

**Potential issue -- NaN coordinates**: Line 92 uses `partial_cmp(...).unwrap()`. If any coordinate is NaN, this panics. For star positions (pixel coordinates), NaN should never occur. The unwrap is acceptable per project conventions but worth noting.

### KNN Search (lines 127-182)

**Algorithm**: Standard recursive KNN with bounded max-heap. Visit the node at `mid`, push its distance, then recurse into the nearer subtree first, then conditionally recurse into the farther subtree.

**Pruning** (line 179): `if !heap.is_full() || diff_sq < heap.max_distance()`. This is the standard condition:
- If the heap is not yet full, we must explore all branches.
- If the squared distance from the query to the splitting plane (`diff_sq`) is less than the current k-th best distance, the farther subtree might contain closer points.

This is **correct**. The comparison uses strict `<` for `diff_sq < heap.max_distance()`, which means points exactly on the boundary of the current best distance are not explored in the farther subtree. This is fine because such points would be at exactly the same distance as the worst current neighbor and would not improve the result set (they would replace equal-distance entries, but the heap already contains k items and the new point is not strictly closer).

**Subtree ordering** (lines 169-173): If `diff < 0.0`, the query is on the left side of the split, so search left first. Otherwise search right first. This is correct and important for pruning efficiency.

**Edge cases handled**:
- `k == 0` returns empty (line 128-130).
- `k > n` returns all n points (tested in `test_kdtree_k_nearest_more_than_available`).
- Empty tree returns empty (line 128).

**Result sorting**: Line 136 sorts output by `dist_sq` ascending. This is a convenience for callers. Cost is O(k log k), negligible for typical k.

### Radius Search (lines 184-233)

**Algorithm**: Recursive traversal, collecting all points within `radius_sq` of the query.

**Pruning** (lines 226-231):
```rust
if diff <= 0.0 || diff_sq <= radius_sq {
    // search left subtree
}
if diff >= 0.0 || diff_sq <= radius_sq {
    // search right subtree
}
```

Where `diff = query_val - point_val` (signed distance from query to splitting plane along the split axis).

**Correctness analysis**:
- If `diff <= 0.0`: query is on the left side (or on the plane), so the left subtree is the "near" side -- always search it. The right subtree is only searched if `diff_sq <= radius_sq` (the radius crosses the split plane).
- If `diff >= 0.0`: query is on the right side (or on the plane), so the right subtree is the "near" side -- always search it. The left subtree is only searched if `diff_sq <= radius_sq`.
- If `diff == 0.0`: both conditions trigger, so both subtrees are always searched. This is correct because the query sits on the splitting plane and both sides could contain points within the radius.

This pruning is **correct** and slightly more conservative than needed when `diff == 0.0` (it searches both unconditionally), but this is the right thing to do.

**Buffer reuse**: `radius_indices_into` takes `&mut Vec<usize>`, clears it, and appends results. This avoids allocation in hot loops (tested in `test_kdtree_radius_indices_buffer_reuse`).

### BoundedMaxHeap (lines 246-389)

**Design**: A custom max-heap that keeps the k smallest distances. Two variants:
- `Small`: stack-allocated array of 32 `Neighbor` entries (common case: k <= 32).
- `Large`: heap-allocated `Vec` for k > 32.

**Heap operations**:
- `push` (lines 293-320): If not full, insert and sift up. If full and new element is smaller than max, replace root and sift down. This is standard bounded-heap insertion.
- `sift_up` (lines 355-365): Standard binary heap sift-up by parent index `(idx - 1) / 2`.
- `sift_down` (lines 367-388): Standard binary heap sift-down comparing left/right children.

**Correctness**: The heap maintains the max-heap property on `dist_sq`. After k insertions, only elements with `dist_sq < items[0].dist_sq` are accepted. This correctly tracks the k nearest points.

**Performance note**: The `Small` variant uses a fixed 32-element array (32 * 16 = 512 bytes on the stack). This avoids heap allocation for the common case. The `#[allow(clippy::large_enum_variant)]` annotation (line 258) correctly suppresses the size difference warning.

## Performance Analysis

### Construction

For n = 200 stars (typical maximum):
- `select_nth_unstable_by` on 200 elements: ~200-400 comparisons.
- Tree depth: ~8 levels.
- Total: ~1600-3200 comparisons. Microsecond-scale on modern CPUs.

### KNN Query

For n = 200, k = 20 (typical for triangle formation):
- Expected nodes visited: O(sqrt(n) + k) in 2D = ~14 + 20 = ~34 nodes.
- Per query: sub-microsecond.
- 200 queries (one per star): still sub-millisecond total.

### Radius Query

For n = thousands of triangle invariants, typical radius tolerance:
- Similar asymptotic behavior. The invariant-space k-d tree handles this well.

### Memory

- `indices`: 200 * 8 bytes = 1.6 KB
- `points`: 200 * 16 bytes = 3.2 KB
- Total: ~5 KB. Fits in L1 cache.

### Brute Force Comparison

For n < 200, brute-force KNN is O(n*k) per query, O(n^2 * k) for all queries. With n=200, k=20: 800,000 comparisons vs k-d tree's ~6,800. The k-d tree wins by ~100x in comparisons, though both are fast enough in practice. The k-d tree approach is the right choice even at this scale because:
1. It is already implemented and correct.
2. It handles the invariant-space queries on thousands of triangles where brute force would be noticeably slower.
3. Removing it would mean different code paths for different uses.

## Issues Found

### No Issues (implementation is solid)

The code is clean, correct, and well-tested. Specific positives:

1. **Implicit layout is correctly implemented** -- build and search use the same `mid` formula.
2. **KNN pruning is standard and correct** -- matches textbook descriptions.
3. **Radius pruning is correct** -- handles the `diff == 0` (on-plane) case properly.
4. **BoundedMaxHeap is correctly implemented** -- standard binary heap with bounded capacity.
5. **Edge cases are thoroughly tested** -- empty tree, k=0, k>n, duplicate points, identical points, collinear points, negative coordinates, zero radius, boundary distance.
6. **Buffer reuse pattern** for radius search avoids allocation in loops.

### Minor Observations (not bugs)

1. **`partial_cmp().unwrap()` on line 92 and line 136**: Will panic on NaN. Acceptable for star coordinates (which should never be NaN), but could be hardened with `total_cmp()` on f64 (available since Rust 1.62) which treats NaN consistently.

2. **Points are cloned** (line 60): `points.to_vec()` copies all points into the tree. For DVec2 (16 bytes each), this is 200 * 16 = 3.2 KB. Negligible. An alternative would be to store a reference `&[DVec2]`, but the owned copy avoids lifetime complexity.

3. **Stack depth during search**: KNN and radius search use recursion. For n=200, depth is ~8. For n=10,000 (unlikely), depth is ~14. No stack overflow risk.

4. **`into_vec` for Small variant** (line 350): Copies from stack array to Vec. This is one allocation per KNN query. Could be avoided by returning an iterator, but the caller (`k_nearest`) sorts the result anyway, so the Vec is needed.

## Potential Improvements

### Not Recommended (overkill for this use case)

1. **Van Emde Boas / Eytzinger layout**: Cache-oblivious layouts improve performance for millions of points. With n <= 200 (fits in L1 cache), there is zero benefit.

2. **SIMD distance computation**: Not worth it for single-point distance checks. The loop body is already simple scalar code.

3. **Approximate NN (FLANN-style)**: Exact NN is needed for star matching correctness. Approximate methods introduce errors that propagate through triangle voting.

4. **External crate (kiddo)**: Adds a dependency for code that is ~390 lines, thoroughly tested, and performant at this scale. The README.md already documents this decision.

### Could Consider

1. **`f64::total_cmp()` instead of `partial_cmp().unwrap()`**: Eliminates the panic path for NaN. Slightly more defensive. One-line change on lines 92 and 136.

2. **Iterative KNN/radius search**: Replace recursion with an explicit stack (as done in construction). Would eliminate any theoretical stack overflow concern for very deep trees. Not needed at current scale.

3. **`get_point` bounds check removal**: `get_point` (line 241-243) uses `self.points[idx]` which panics on out-of-bounds. All callers use valid indices from the tree itself, so this is fine. Adding `debug_assert!(idx < self.points.len())` would be more explicit.

## References

- [Implicit k-d tree (Wikipedia)](https://en.wikipedia.org/wiki/Implicit_k-d_tree)
- [sif-kdtree crate (flat implicit layout in Rust)](https://docs.rs/sif-kdtree/latest/sif_kdtree/)
- [Cache-oblivious k-d tree (GeeksforGeeks)](https://www.geeksforgeeks.org/dsa/cache-oblivious-kd-tree/)
- [Eytzinger Binary Search (Algorithmica)](https://algorithmica.org/en/eytzinger)
- [K-D Tree vs Ball Tree comparison (GeeksforGeeks)](https://www.geeksforgeeks.org/machine-learning/ball-tree-and-kd-tree-algorithms/)
- [Rust select_nth_unstable O(n^2) fix (PR #107522)](https://github.com/rust-lang/rust/pull/107522)
- [Astrometry.net k-d tree (libkd)](https://astrometry.net/doc/libkd.html)
- [Astrometry.net blind calibration paper](https://arxiv.org/pdf/0910.2233)
- [Global Multi-Triangle Voting star identification](https://www.mdpi.com/2076-3417/12/19/9993)
- [scikit-learn Nearest Neighbors (brute vs tree crossover)](https://scikit-learn.org/stable/modules/neighbors.html)
- [FLANN approximate nearest neighbors](https://www.cs.ubc.ca/research/flann/)
