# Spatial Module (K-D Tree)

## Architecture

A custom 2D k-d tree in ~450 lines (`mod.rs`) for spatial queries on star positions. Three structs:

- `KdTree` -- flat implicit-layout tree storing `indices: Vec<usize>` (permuted point indices) and `points: Vec<DVec2>` (owned copy of input).
- `Neighbor` -- query result: `index: usize` + `dist_sq: f64`.
- `BoundedMaxHeap` -- bounded max-heap for KNN. Two variants: `Small` (stack-allocated array, k <= 32) and `Large` (heap-allocated Vec).

Tests in `tests.rs` (~870 lines, 30 tests).

### Usage in Pipeline

1. **Triangle formation** (`triangle/matching.rs:113`) -- KdTree on star positions, `k_nearest` queries to form triangles. Typical n: 50-200 stars, k: 5-20.
2. **Invariant-space matching** (`triangle/voting.rs:96-101`) -- KdTree on triangle ratio pairs (2D), `radius_indices_into` for similar triangles. Typical n: hundreds to thousands of triangles. Uses L2 radius scaled by sqrt(2) to circumscribe L-infinity tolerance, then post-filters with `is_similar()`.
3. **Match recovery** (`registration/mod.rs:419`) -- KdTree on target stars, `nearest_one` to find additional inlier matches after RANSAC.

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
| Squared distances | Yes (avoids sqrt) | Yes (avoids sqrt) |

Key differences:
- nanoflann's leaf-based approach reduces function call overhead: instead of recursing down to single elements, it brute-forces within groups of 10-50 points. This is a meaningful optimization for large trees (>1000 points) because it reduces branch mispredictions and function call overhead. For our typical n=200, the tree depth is only ~8, so the benefit is negligible. Research (scikit-learn) shows the optimal leaf size depends heavily on data distribution, with defaults of 10 (nanoflann), 16 (scipy), and 30 (scikit-learn). There is a distinct minimum in query time as a function of leaf size; too small increases tree traversal overhead, too large increases brute-force cost.
- nanoflann chooses the split dimension with maximum spread rather than cycling. This produces more balanced spatial regions (squarer cells) when data has anisotropic distribution. Max-spread tends to promote square regions because after splitting on the widest dimension, that dimension is unlikely to remain the widest at the next level. For 2D star positions (roughly uniform scatter on the image), alternating dimensions works well. For the invariant-space tree (triangle ratios concentrated in a narrow band), max-spread could help.

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

kiddo's modified van Emde Boas layout requires ~10 integer ops per node traversal (including one divide) vs one shift for Eytzinger and zero for our implicit layout. The vEB layout is only 1% faster to 5% slower than Eytzinger in kiddo's benchmarks, confirming that the cache improvement does not offset the index computation cost at moderate tree sizes.

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

### Code Quality Issues (not bugs)

1. **`capacity + 1` allocation in Large heap** (`mod.rs:349`): `Vec::with_capacity(capacity + 1)` allocates one extra slot that is never used. The push logic never exceeds `capacity` elements (it replaces the root when full, it does not push-then-pop). The `+ 1` suggests an abandoned insert-then-pop pattern that was never implemented. Should be `Vec::with_capacity(capacity)`. See REVIEW.md [F17].

2. **Heap allocation before empty check** (`mod.rs:128-132`): `k_nearest` creates `BoundedMaxHeap::new(k)` before checking `self.indices.is_empty()`. The heap allocation is wasted for empty trees. Trivial fix: move the empty check first. See REVIEW.md [F18].

3. **Duplicated dimension extraction** (`mod.rs:82-92,164-165,228-229,282-283`): The pattern `if split_dim == 0 { p.x } else { p.y }` appears 8 times across 4 methods. Should be extracted to an `#[inline] fn dim_value(p: DVec2, dim: usize) -> f64`. See REVIEW.md [F9].

4. **Duplicated near/far subtree selection** (`mod.rs:169-173,232-236`): The 4-tuple construction for first/second subtree ranges is identical in `k_nearest_range` and `nearest_one_range`. Could share a helper.

## What We Do Well (matches or exceeds industry standards)

1. **Flat implicit layout** -- Our implicit array layout is more memory-efficient than nanoflann's explicit node structure (8 bytes/node vs ~24-32 bytes/node with child pointers and split value). This also provides better cache locality for small trees.

2. **Squared distance throughout** -- Same optimization as nanoflann: all distance comparisons use squared L2 distance, avoiding sqrt entirely. This is the industry-standard approach.

3. **Iterative construction** -- Construction uses an explicit work stack, which is robust against stack overflow for any input size. Search uses recursion, but with tree depth ~8-14 for our data sizes, this is safe and benefits from better branch prediction in dense trees.

4. **Stack-allocated heap for small k** -- The `BoundedMaxHeap::Small` variant avoids heap allocation for k <= 32 (the common case). This eliminates allocation overhead in the hot path. Most implementations (including kiddo) use heap-allocated vectors unconditionally.

5. **Buffer-reuse radius API** -- `radius_indices_into` takes a `&mut Vec<usize>` buffer, avoiding per-query allocation in the voting loop. This is a practical optimization that most libraries lack (nanoflann returns a new vector each time).

6. **Dedicated `nearest_one`** -- Avoids heap entirely for the 1-NN case, using a scalar best tracker. This is the same optimization kiddo provides.

7. **Median split** -- Produces a perfectly balanced tree. Consistent O(n log n) construction, O(log n) query. The alternative (midpoint split, used by nanoflann) can produce unbalanced trees for clustered data.

## What We Don't Do But Should (missing industry-standard features)

### Recommended

1. **L-infinity (Chebyshev) radius search** -- The only caller of `radius_indices_into` in the hot path (`voting.rs:124-138`) uses L2 radius search scaled by sqrt(2) to circumscribe the L-infinity tolerance square, then post-filters with `is_similar()` (L-infinity check: `|dr0| < tol && |dr1| < tol`). A native `radius_chebyshev_into()` method would:
   - Eliminate the post-filter step entirely.
   - Use simpler pruning: `|diff| <= radius` on the split axis (no squaring needed, one fewer multiply per node).
   - Reduce candidate count by ~21.5% (circle area / square area = pi/4 ~ 0.785, so ~21.5% of L2-radius candidates are outside the L-infinity square and must be post-filtered).
   - The k-d tree splitting plane intersection test for L-infinity is trivially `|diff| <= radius`, which is the same check needed per-axis anyway.
   - Industry support: scipy, scikit-learn, and MATLAB all provide L-infinity distance metrics for k-d tree queries natively.

### Not Recommended (overkill for current scale)

2. **Leaf-size optimization**: Switch to brute-force at 16-32 elements per leaf. Industry standard (nanoflann: 10, libkd: 32, scikit-learn: 30, scipy: 16, kiddo: varies). Research shows an optimal leaf size exists where tree traversal overhead and brute-force cost are balanced. Would reduce function call overhead by ~3x for the invariant-space tree (n = 1000-5000). For the star tree (n = 200), no measurable benefit. Only worth implementing if profiling shows the invariant-space tree queries are a bottleneck.

3. **Eytzinger / van Emde Boas layout**: Cache-oblivious layouts improve performance for trees that do not fit in L1 cache. Research (Khuong & Morin 2015) shows sorted order is best for small n, and Eytzinger becomes faster for large n. Our tree fits in L1 (5 KB at n=200, ~80 KB at n=5000). kiddo's benchmarks show vEB is only 0-5% faster than Eytzinger, with more complex index arithmetic. No benefit at current scale.

4. **Max-spread split dimension**: Choose the coordinate with the largest range instead of cycling. Produces more balanced spatial regions (squarer cells) for anisotropic distributions. Amenta et al. showed this reduces the number of leaf nodes inspected during NN search. The invariant-space data (triangle ratios) could benefit, but the improvement is marginal for 2D. Adds O(n) per level to compute spread.

5. **Iterative KNN/radius search**: Replace recursion with an explicit stack (as done in construction). Research shows recursive approaches have better branch prediction for dense trees, while iterative approaches are safer for very deep trees. For our depth 8-14, recursion is correct. Negligible difference.

6. **Distance metric abstraction**: Support L1, L2, L-infinity via a trait or enum. Only L-infinity is useful (see item 1). A dedicated `radius_chebyshev_into()` method is simpler than a generic metric system.

7. **SIMD distance computation**: Not worth it for single-point distance checks in 2D. The loop body is one subtraction + one dot product. kiddo's SIMD support is for f64 `nearest_one` on nightly only.

8. **Approximate NN (FLANN-style)**: Exact NN is required for star matching correctness.

9. **d-ary heap**: Research suggests 4-ary heaps can outperform binary heaps due to cache locality. For k <= 32 (stack-allocated), the difference is negligible since the entire heap is L1-resident.

## What We Do That's Not Needed (unnecessary complexity)

Nothing significant. The implementation is lean at ~450 lines for the full module. The `BoundedMaxHeap::Large` variant is needed for the edge case of k > 32, which could theoretically arise. The `Small/Large` enum distinction adds ~30 lines of code but provides real allocation savings for the common case.

## Performance Considerations

- **Construction**: For n=200, construction takes ~1-2 microseconds. Not a bottleneck.
- **KNN query**: For n=200, k=20, a single query visits ~30-50 nodes (tree depth 8, searching near subtree fully plus pruned far subtrees). Sub-microsecond.
- **Radius query**: For the invariant-space tree (n=1000-5000), radius queries are the hot path in voting. Each query returns a small number of candidates (typically 0-10). The L2-to-L-infinity scaling wastes ~22% of traversal effort on candidates that are filtered out.
- **Memory**: The tree stores `indices: Vec<usize>` (8 bytes/point) + `points: Vec<DVec2>` (16 bytes/point) = 24 bytes/point. For 200 points: 4.8 KB. For 5000 points (invariant tree): 120 KB. Both fit comfortably in L2 cache.
- **Recursion depth**: log2(200) = 7.6, log2(5000) = 12.3. Well within stack limits.

## Research Sources

- [nanoflann C++ library -- GitHub](https://github.com/jlblancoc/nanoflann) -- Header-only KD-tree with leaf_max_size, max-spread split, CRTP adaptor pattern.
- [nanoflann API documentation](https://jlblancoc.github.io/nanoflann/) -- Node structure, leaf behavior, radius search API.
- [nanoflann KDTree classes reference](https://jlblancoc.github.io/nanoflann/group__kdtrees__grp.html) -- KDTreeSingleIndexAdaptor details.
- [scipy.spatial.KDTree -- SciPy v1.17.0](https://docs.scipy.org/doc/scipy/reference/generated/scipy.spatial.KDTree.html) -- Sliding midpoint rule, balanced_tree option, compact_nodes, L-infinity support.
- [scipy.spatial.cKDTree -- SciPy v1.17.0](https://docs.scipy.org/doc/scipy/reference/generated/scipy.spatial.cKDTree.html) -- C implementation details, Maneewongvatana & Mount 1999 reference.
- [scikit-learn KDTree](https://scikit-learn.org/stable/modules/generated/sklearn.neighbors.KDTree.html) -- query_radius with metric parameter, leaf_size=30 default, L-infinity support.
- [scikit-learn Nearest Neighbors documentation](https://scikit-learn.org/stable/modules/neighbors.html) -- Brute vs tree crossover analysis, leaf_size parameter, optimal leaf size discussion.
- [FLANN -- Fast Library for Approximate Nearest Neighbors](https://www.cs.ubc.ca/research/flann/) -- Randomized k-d forests, hierarchical k-means, auto-configuration.
- [Astrometry.net libkd documentation](https://astrometry.net/doc/libkd.html) -- KD_BUILD_BBOX vs KD_BUILD_SPLIT, Nleaf parameter, FITS I/O.
- [Astrometry.net blind calibration paper (arXiv:0910.2233)](https://arxiv.org/pdf/0910.2233) -- Star matching with geometric hashing, k-d tree acceleration.
- [kiddo Rust crate -- GitHub](https://github.com/sdd/kiddo) -- Eytzinger and modified van Emde Boas layouts, SIMD nearest_one.
- [kiddo crate documentation](https://docs.rs/kiddo/latest/kiddo/) -- Cache-line analysis: one fetch per 3 levels (f64 Eytzinger), per 4 levels (f32 vEB). vEB is 1% faster to 5% slower than Eytzinger.
- [Array Layouts for Comparison-Based Searching (Khuong & Morin 2015)](https://arxiv.org/pdf/1509.05053) -- Eytzinger vs vEB vs sorted: Eytzinger fastest for large n, sorted best for small n.
- [K-D Trees Are Better when Cut on the Longest Side (Amenta et al.)](https://web.cs.ucdavis.edu/~amenta/w07/kdlongest.pdf) -- Max-spread split produces better spatial balance, squarer cells.
- [k-d tree -- Wikipedia](https://en.wikipedia.org/wiki/K-d_tree) -- Standard algorithm description, pruning conditions.
- [Implicit k-d tree -- Wikipedia](https://en.wikipedia.org/wiki/Implicit_k-d_tree) -- Flat array layout, child index formulas, memory advantages.
- [Chebyshev distance -- Wikipedia](https://en.wikipedia.org/wiki/Chebyshev_distance) -- L-infinity metric definition, square "circles".
- [Rust select_nth_unstable O(n^2) fix (PR #107522)](https://github.com/rust-lang/rust/pull/107522) -- Median-of-medians fallback guarantees O(n) worst case.
- [CMSC 420: Answering Queries with kd-trees (Dave Mount)](https://www.cs.umd.edu/class/fall2019/cmsc420-0201/Lects/lect14-kd-query.pdf) -- Formal pruning analysis for range and NN queries.
- [Efficient Radius Neighbor Search in Three-dimensional Point Clouds (Behley et al. 2015)](https://jbehley.github.io/papers/behley2015icra.pdf) -- Radius search optimizations for point clouds.
- [Cache-Friendly Search Trees (Holm 2019)](https://arxiv.org/pdf/1907.01631) -- Eytzinger layout cache analysis, comparison with sorted arrays.
- [Recursive vs Iterative DFS Performance](https://medium.com/@npiontko/recursive-vs-iterative-dfs-performance-surprise-12327cd8a67d) -- Branch prediction advantages of recursion in dense trees.
- [scipy split dimension variance discussion (#11941)](https://github.com/scipy/scipy/issues/11941) -- Max-variance split for scipy KDTree.
- [Benchmarking Nearest Neighbor Searches in Python (VanderPlas 2013)](https://jakevdp.github.io/blog/2013/04/29/benchmarking-nearest-neighbor-searches-in-python/) -- leaf_size optimization, distinct performance minimum.
- [MATLAB KDTreeSearcher](https://www.mathworks.com/help/stats/kdtreesearcher.html) -- Chebyshev distance support for KD-tree queries.
