# Lumos Star Detection Optimization Plan

This document outlines a plan to implement performance improvements for the `star-detection` module in the `lumos` crate, based on the findings from the benchmark analysis.

The goal is to improve performance by targeting the most significant bottlenecks: background estimation and source deblending.

---

## Phase 1: Optimize Background Estimation

**Objective:** The benchmark report noted regressions and suggested parallelization. A code review shows that tile-stat generation and interpolation are already parallelized using `rayon`. However, the `apply_median_filter` function, which runs on the tile grid, is currently serial. This plan focuses on parallelizing it.

**Files to Modify:**
*   `lumos/src/star_detection/background/mod.rs`

**Implementation Steps:**

1.  Navigate to the `apply_median_filter` function within `lumos/src/star_detection/background/mod.rs`.
2.  The outer loops (`for ty in 0..self.tiles_y` and `for tx in 0..self.tiles_x`) can be combined and parallelized.
3.  The results will be collected into a `filtered` `Vec`. Since the write position `ty * self.tiles_x + tx` is unique for each item, this is safe to do in parallel.
4.  Convert the iterator to a parallel one and collect the results:
    ```rust
    // Before
    let mut filtered = vec![...];
    for ty in 0..self.tiles_y {
        for tx in 0..self.tiles_x {
            // ... logic ...
            filtered[ty * self.tiles_x + tx] = ...;
        }
    }
    self.stats = filtered;

    // After (Proposed)
    let filtered_stats: Vec<TileStats> = (0..self.tiles_y * self.tiles_x)
        .into_par_iter()
        .map(|idx| {
            let ty = idx / self.tiles_x;
            let tx = idx % self.tiles_x;
            // ... existing logic to compute filtered_median and filtered_sigma ...
            TileStats { median: filtered_median, sigma: filtered_sigma }
        })
        .collect();
    self.stats = filtered_stats;
    ```

---

## Phase 2: Parallelize Multi-Threshold Deblending

**Objective:** The benchmark analysis identified deblending as a major bottleneck in crowded fields (~2ms per pair). Parallelizing the deblending of independent source groups will provide a significant speedup.

**Files to Modify:**
*   `lumos/src/star_detection/deblend/mod.rs` (and related files in that module).

**Implementation Steps:**

1.  Locate the primary deblending function that receives a list of blended sources (groups of overlapping peaks).
2.  Instead of iterating through this list of source groups serially, use `rayon`'s `into_par_iter()`.
3.  The `map` operation will perform the deblending for a single group, returning a `Vec` of new, separated `Star` objects.
4.  Use `flatten()` or a similar reducer to combine the `Vec<Vec<Star>>` results from all the parallel tasks into a single `Vec<Star>`.

    ```rust
    // Fictional example of the change
    // Before
    let mut deblended_stars = Vec::new();
    for group in blended_groups {
        deblended_stars.extend(deblend_one_group(group, &image));
    }

    // After (Proposed)
    let deblended_stars: Vec<Star> = blended_groups
        .into_par_iter()
        .flat_map(|group| deblend_one_group(group, &image))
        .collect();
    ```

---

## Verification Plan

For each phase, the following steps must be taken to verify both correctness and performance improvement.

### 1. Benchmarking

Run the specific benchmarks before and after your changes to quantify the impact.

*   **Command:** `cargo bench --bench <name>`
*   **Background Estimation Benchmark:**
    *   Run: `cargo bench --bench star_detection_background`
    *   Analyze: `background_estimation/*` results. Expect a reduction in time, especially for larger images where `tiles_x * tiles_y` is large.
*   **Deblending Benchmark:**
    *   Run: `cargo bench --bench star_detection_deblend`
    *   Analyze: `deblend_local_maxima/*` and any multi-threshold deblending benchmarks. Expect significant time reduction on multi-core systems.

### 2. Testing

Ensure that the optimized implementation produces numerically identical results to the original.

*   **Command:** `cargo nextest run -p lumos` (The project is configured to use nextest).
*   **Correctness:** All existing tests related to `star_detection` must pass.
*   **Order-Independence:** The parallelized deblending may output stars in a different order. If tests fail due to ordering, they should be updated to be order-independent (e.g., by sorting the expected and actual star lists by position before comparison). No new tests should be required if the logic is preserved.
