# Spatial Module (K-D Tree)

## Architecture

A custom 2D k-d tree in ~450 lines (`mod.rs`) for spatial queries on star positions. Three structs:

- `KdTree` -- flat implicit-layout tree storing `indices: Vec<usize>` (permuted point indices) and `points: Vec<DVec2>` (owned copy of input).
- `Neighbor` -- query result: `index: usize` + `dist_sq: f64`.
- `BoundedMaxHeap` -- bounded max-heap for KNN. Two variants: `Small` (stack-allocated array, k <= 32) and `Large` (heap-allocated Vec).

Tests in `tests.rs` (~570 lines, 25 tests).

### Usage in Pipeline

1. **Triangle formation** (`triangle/matching.rs:20`) -- KdTree on star positions, `k_nearest` queries to form triangles. Typical n: 50-200 stars, k: 5-20.
2. **Invariant-space matching** (`triangle/voting.rs:96-101`) -- KdTree on triangle ratio pairs (2D), `radius_indices_into` for similar triangles. Typical n: hundreds to thousands of triangles.
3. **Match recovery** (`registration/mod.rs:361,390`) -- KdTree on target stars, `nearest_one` to find additional inlier matches after RANSAC.

## Algorithm Description

### Construction (`mod.rs:55-117`)

Iterative median-split using an explicit work stack. For each range `[start, end)`:
1. Compute `median = len / 2`.
2. Call `select_nth_unstable_by(median, ...)` -- Rust's introselect, O(n) guaranteed since Rust 1.77.
3. The median element becomes the node at `mid = start + median`.
4. Left subtree is `[start, mid)`, right subtree is `[mid+1, end)`.
5. Split dimension alternates by depth: `depth % 2` (0=x, 1=y).

Complexity: O(n log n) time, O(n) space. Comparison uses `f64::total_cmp()` (NaN-safe total ordering).

### KNN Search (`mod.rs:127-182`)

Standard recursive best-first search with bounded max-heap:
1. Visit node at `mid`, push to heap.
2. Determine near/far subtrees based on `diff = query_val - point_val`.
3. Recurse into near subtree first.
4. Prune far subtree if `heap.is_full() && diff_sq >= heap.max_distance()`.
5. Sort final results by `dist_sq` ascending.

### Nearest-One Search (`mod.rs:188-243`)

Specialized 1-NN with scalar `best` tracker instead of heap. Same near-first/prune-far logic. No allocation.

### Radius Search (`mod.rs:249-294`)

Recursive traversal collecting all points within `radius_sq`:
1. Visit node, check `dist_sq <= radius_sq`.
2. Prune subtrees: always search the near side; search the far side only if `diff_sq <= radius_sq`.

Buffer-reuse API: `radius_indices_into(&self, query, radius, &mut Vec<usize>)` clears and fills the buffer. Avoids allocation in hot loops.

### BoundedMaxHeap (`mod.rs:307-450`)

Custom max-heap tracking k smallest distances. Stack-allocated for k <= 32 (512 bytes), heap-allocated otherwise. Standard sift-up/sift-down operations.

## Comparison with Industry Standard Implementations

### vs nanoflann (C++, the dominant point cloud KD-tree)

| Aspect | Our KdTree | nanoflann |
|--------|-----------|-----------|
| Node structure | Implicit flat array (8 bytes/node) | Explicit nodes with child pointers + split value |
| Leaf handling | No leaf nodes; recurse to individual points | leaf_max_size (default 10), brute-force within leaves |
| Split dimension | Alternating `depth % 2` | Max-spread dimension (widest coordinate range) |
| Split value | Median element | Midpoint of bounding box, clamped to data range |
| Distance metrics | L2 only | Pluggable (L1, L2, custom) |
| Radius search | Collects indices only | Returns (index, distance) pairs |
| Memory model | Owns points (copies input) | Adaptor pattern, zero-copy from user data |

Key differences:
- nanoflann's leaf-based approach reduces function call overhead: instead of recursing down to single elements, it brute-forces within groups of 10-50 points. This is a meaningful optimization for large trees (>1000 points) because it reduces branch mispredictions and function call overhead. For our typical n=200, the tree depth is only ~8, so the benefit is negligible.
- nanoflann chooses the split dimension with maximum spread rather than cycling. This produces more spatially balanced splits when data has anisotropic distribution. For 2D star positions (roughly uniform scatter on the image), alternating dimensions works well. For the invariant-space tree (triangle ratios concentrated in a narrow band), max-spread could help.

### vs scipy.spatial.KDTree (Python/C, the reference scientific implementation)

| Aspect | Our KdTree | scipy KDTree |
|--------|-----------|-------------|
| Split strategy | Alternating + median | Sliding midpoint (Maneewongvatana & Mount 1999) or median split |
| Compact nodes | No (implicit layout) | Optional: shrinks bounding boxes to actual data range |
| Distance metrics | L2 only | L1, L2, L-infinity, Minkowski |
| balanced_tree option | Always balanced (median) | Configurable; default True since v1.4 |
| Leaf size | 1 (no leaf optimization) | Default 16 |

scipy's sliding midpoint rule prevents degenerate thin cells. Not needed for 2D astronomical data which is typically well-distributed.

### vs Astrometry.net libkd (C, astronomical application)

| Aspect | Our KdTree | libkd |
|--------|-----------|-------|
| Domain | 2D pixel coordinates | 2D-3D celestial coordinates |
| Split strategy | Alternating + median | Configurable (SPLIT or full BBOX per node) |
| Leaf size | 1 | Configurable (Nleaf parameter) |
| Serialization | None | FITS format I/O |
| Data types | f64 only | f64, f32, u16, u32 |

libkd is designed for persistent indexes (million-star catalogs stored on disk). Our tree is ephemeral (built per-image, <200 stars). The design tradeoffs are appropriate for each use case.

### vs kiddo (Rust, the dominant Rust KD-tree crate)

| Aspect | Our KdTree | kiddo v5 |
|--------|-----------|----------|
| Layout | Flat implicit (in-order) | Eytzinger (BFS order) or modified van Emde Boas |
| Dimensions | 2D hardcoded | Const-generic N-dimensional |
| SIMD | None | Optional (nightly, for `nearest_one` on f64) |
| Serialization | None | rkyv zero-copy |
| Cache optimization | None (L1-resident at n=200) | Eytzinger reduces cache-line fetches for deep trees |

kiddo's Eytzinger layout fetches a new cache line approximately every 3 levels (for f64). Since our tree depth is ~8 levels, that is ~3 cache misses in the worst case, compared to ~8 with the in-order layout. At n=200 the entire tree (5 KB) fits in L1 cache, making this irrelevant. For the invariant-space tree (thousands of points, ~80 KB), the difference is also small because L2 cache (256 KB) absorbs it.

### vs FLANN (C++, approximate NN for high-dimensional data)

Not applicable. FLANN targets high-dimensional approximate NN using randomized k-d forests and hierarchical k-means. Our 2D exact queries are a trivial case for any exact k-d tree. Approximate methods would introduce errors in star matching that propagate through triangle voting.

## Issues Found

### No Correctness Bugs

The implementation is correct. Specific verification:

1. **Build/search index consistency** (`mod.rs:78,95` vs `mod.rs:153`): Build uses `mid = range.start + median` where `median = len / 2`. Search uses `mid = start + (end - start) / 2`. These are algebraically identical. Verified.

2. **KNN pruning** (`mod.rs:179`): `!heap.is_full() || diff_sq < heap.max_distance()`. Strict `<` is correct: when the heap is full and a candidate's splitting-plane distance equals the worst distance, it cannot improve the result. Matches textbook algorithm.

3. **Radius pruning** (`mod.rs:287-293`): The `diff <= 0.0` / `diff >= 0.0` conditions correctly identify the near subtree (always searched) and far subtree (searched only if `diff_sq <= radius_sq`). The `diff == 0.0` case correctly searches both subtrees unconditionally.

4. **nearest_one pruning** (`mod.rs:240`): Uses `diff * diff < best.dist_sq` (strict `<`). If a point exactly on the splitting plane is equidistant to the current best, we skip the far subtree. This is fine because the splitting-plane distance is a lower bound; even if we cross, no point on the other side can be strictly closer.

5. **BoundedMaxHeap** (`mod.rs:334-450`): Standard binary max-heap with bounded capacity. `sift_up` and `sift_down` are textbook correct. `push` correctly handles the three cases: heap not full (insert + sift up), heap full and new element smaller (replace root + sift down), heap full and new element larger (reject).

### Minor Observations (not bugs, no action needed)

1. **Points are cloned** (`mod.rs:60`): `points.to_vec()` copies all points into the tree. For DVec2 (16 bytes each), this is 200 * 16 = 3.2 KB. Negligible. An alternative would be to store a reference `&[DVec2]`, but the owned copy avoids lifetime complexity. Correct design choice.

2. **Stack depth during search**: KNN and radius search use recursion. For n=200, depth is ~8. For n=10,000, depth is ~14. No stack overflow risk.

3. **`into_vec` for Small variant** (`mod.rs:409-411`): Copies from stack array to Vec. This is one allocation per KNN query. Could be avoided by returning an iterator, but the caller (`k_nearest`) sorts the result anyway, so the Vec is needed.

4. **Alternating dimension strategy**: The `depth % 2` cycling is simple and correct for 2D. The max-spread strategy (used by nanoflann) would be slightly better for anisotropic distributions but adds complexity and a per-level O(n) scan to compute spread. Not worth it for 2D.

## Prioritized Improvements

### Recommended

1. **L-infinity radius search** -- The invariant-space voting code (`voting.rs:124-138`) uses L2 radius search scaled by sqrt(2) to circumscribe the L-infinity tolerance square, then post-filters with `is_similar()` (L-infinity check). A native `radius_chebyshev_into()` method would:
   - Eliminate the post-filter.
   - Use simpler pruning: `|diff| <= radius` on the split axis (no squaring needed).
   - Reduce candidate count by ~22% (circle area vs square area = pi/4 ~ 0.785, so ~21.5% of candidates are in the circle but outside the square).
   - The k-d tree splitting plane intersection test for L-infinity is simpler than for L2: check `|diff| <= radius` on the split axis, which is exactly the pruning condition already needed.

### Not Recommended (overkill for current scale)

2. **Leaf-size optimization**: Switch to brute-force at 16-32 elements per leaf. Industry standard (nanoflann: 10, libkd: 32, scikit-learn: 16, kiddo: varies). Would reduce function call overhead by ~3x for the invariant-space tree (n = 1000-5000). For the star tree (n = 200), no measurable benefit. Only worth implementing if profiling shows the invariant-space tree queries are a bottleneck.

3. **Eytzinger / van Emde Boas layout**: Cache-oblivious layouts improve performance for trees that do not fit in L1 cache. Research (Khuong & Morin 2015) shows sorted order is best for small n, and Eytzinger becomes faster for large n. Our tree fits in L1 (5 KB at n=200, ~80 KB at n=5000). No benefit at current scale.

4. **Max-spread split dimension**: Choose the coordinate with the largest range instead of cycling. Produces more balanced spatial regions for anisotropic distributions. The invariant-space data (triangle ratios) could benefit, but the improvement is marginal for 2D. Adds O(n) per level to compute spread.

5. **Iterative KNN/radius search**: Replace recursion with an explicit stack (as done in construction). Avoids function call overhead. Negligible for depth ~8-14.

6. **Distance metric abstraction**: Support L1, L2, L-infinity via a trait or enum. Only L-infinity is useful (see item 1). A dedicated `radius_chebyshev_into()` method is simpler than a generic metric system.

7. **SIMD distance computation**: Not worth it for single-point distance checks in 2D. The loop body is one subtraction + one dot product.

8. **Approximate NN (FLANN-style)**: Exact NN is required for star matching correctness.

## Research Sources

- [nanoflann C++ library -- GitHub](https://github.com/jlblancoc/nanoflann) -- Header-only KD-tree with leaf_max_size, max-spread split, CRTP adaptor pattern.
- [nanoflann API documentation](https://jlblancoc.github.io/nanoflann/) -- Node structure, leaf behavior, radius search API.
- [scipy.spatial.KDTree -- SciPy v1.17.0](https://docs.scipy.org/doc/scipy/reference/generated/scipy.spatial.KDTree.html) -- Sliding midpoint rule, balanced_tree option, compact_nodes.
- [scipy.spatial.cKDTree -- SciPy v1.17.0](https://docs.scipy.org/doc/scipy/reference/generated/scipy.spatial.cKDTree.html) -- C implementation details, Maneewongvatana & Mount 1999 reference.
- [FLANN -- Fast Library for Approximate Nearest Neighbors](https://www.cs.ubc.ca/research/flann/) -- Randomized k-d forests, hierarchical k-means, auto-configuration.
- [Astrometry.net libkd documentation](https://astrometry.net/doc/libkd.html) -- KD_BUILD_BBOX vs KD_BUILD_SPLIT, Nleaf parameter, FITS I/O.
- [Astrometry.net blind calibration paper (arXiv:0910.2233)](https://arxiv.org/pdf/0910.2233) -- Star matching with geometric hashing, k-d tree acceleration.
- [kiddo Rust crate -- GitHub](https://github.com/sdd/kiddo) -- Eytzinger and modified van Emde Boas layouts, SIMD nearest_one.
- [kiddo crate documentation](https://docs.rs/kiddo/latest/kiddo/) -- Cache-line analysis: one fetch per 3 levels (f64 Eytzinger), per 4 levels (f32 vEB).
- [Array Layouts for Comparison-Based Searching (Khuong & Morin 2015)](https://arxiv.org/pdf/1509.05053) -- Eytzinger vs vEB vs sorted: Eytzinger fastest for large n, sorted best for small n.
- [K-D Trees Are Better when Cut on the Longest Side (Amenta et al.)](https://web.cs.ucdavis.edu/~amenta/w07/kdlongest.pdf) -- Max-spread split produces better spatial balance.
- [k-d tree -- Wikipedia](https://en.wikipedia.org/wiki/K-d_tree) -- Standard algorithm description, pruning conditions.
- [Rust select_nth_unstable O(n^2) fix (PR #107522)](https://github.com/rust-lang/rust/pull/107522) -- Median-of-medians fallback guarantees O(n) worst case.
- [scikit-learn Nearest Neighbors documentation](https://scikit-learn.org/stable/modules/neighbors.html) -- Brute vs tree crossover analysis, leaf_size parameter.
- [CMSC 420: Answering Queries with kd-trees (Dave Mount)](https://www.cs.umd.edu/class/fall2019/cmsc420-0201/Lects/lect14-kd-query.pdf) -- Formal pruning analysis for range and NN queries.
- [Efficient Radius Neighbor Search in Three-dimensional Point Clouds (Behley et al. 2015)](https://jbehley.github.io/papers/behley2015icra.pdf) -- Radius search optimizations for point clouds.
