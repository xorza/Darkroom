# Star Detection Performance Analysis

**Date:** 2026-02-02  
**Image:** rho-opiuchi.jpg (8584x5874, ~50M pixels)  
**Config:** AdaptiveSigma, n_thresholds=64, 8-connectivity  
**Benchmark:** ~1.6s per detection (10 iterations)

## Profile Summary

| Function | Self % | Total % | Notes |
|----------|--------|---------|-------|
| `deblend_multi_threshold` | 19.39% | **41.42%** | Main bottleneck |
| `visit_neighbors_grid` | 28.57% | 28.65% | BFS neighbor traversal |
| `find_connected_regions_grid` | 5.84% | 24.06% | Connected component finding |
| `refine_centroid` | 6.05% | 6.08% | Centroid refinement |
| `remove_duplicate_stars` | 5.92% | 5.93% | O(n²) duplicate removal |
| `__expf_fma` | 4.45% | 4.46% | Exponential in Gaussian weights |
| `PixelGrid::reset_with_pixels` | 3.35% | 3.36% | Grid reset per threshold |
| `convolve_row_avx2` | 0.94% | 2.34% | Convolution (SIMD optimized) |
| `process_words_filtered_sse` | 1.36% | 1.37% | Threshold mask (SIMD optimized) |
| `compute_metrics` | 1.33% | 1.33% | Star metric computation |
| Memory allocation (malloc/realloc) | ~2.5% | - | Vec resizing |
| Page faults (clear_page_erms) | ~2% | - | Memory clearing |

## Hotspot Analysis

### 1. Multi-threshold Deblending (41.42% total)

The deblending algorithm is the dominant bottleneck. It's called per-candidate with `n_thresholds=64` levels.

**Root cause:** For each of 64 threshold levels:
1. Filter pixels above threshold → O(n)
2. Find connected regions via BFS → O(n) but with grid overhead
3. Track parent-child relationships

**`visit_neighbors_grid` (28.57% self):**
```rust
fn visit_neighbors_grid(pos: Vec2us, grid: &mut PixelGrid, queue: &mut Vec<Pixel>) {
    // Check all 8 neighbors - unrolled manually
    if y > 0 {
        if x > 0 { try_visit_neighbor(x - 1, y - 1, grid, queue); }
        try_visit_neighbor(x, y - 1, grid, queue);
        try_visit_neighbor(x + 1, y - 1, grid, queue);
    }
    // ... 5 more neighbors
}
```

This function is called millions of times during BFS traversal. Each call does:
- 8 bounds checks
- 8 grid lookups (`grid.get()` + `grid.mark_visited()`)
- Conditional queue pushes

**Optimization opportunities:**

1. **Reduce n_thresholds:** Currently 64. SExtractor default is 32. For this image, 16-32 may suffice.
   - Expected impact: ~50% reduction in deblend time (20% overall)

2. **Early termination:** Skip remaining thresholds if no new splits found for N consecutive levels.

3. **Batch neighbor processing:** Instead of 8 separate function calls, process all neighbors in a single inline block with SIMD gather.

4. **Grid layout optimization:** Current grid uses separate `values` and `visited` arrays. Interleaving them could improve cache locality.

5. **Union-Find instead of BFS:** Replace BFS-based connected components with union-find which has better cache behavior for repeated queries.

### 2. `PixelGrid::reset_with_pixels` (3.35%)

Called once per threshold level (64 times per candidate):
```rust
fn reset_with_pixels(&mut self, pixels: &[Pixel]) {
    self.values.clear();
    self.values.resize(size, NO_PIXEL);  // Memset
    self.visited.clear();
    self.visited.resize(visited_words, 0);  // Memset
    // Populate grid
    for p in pixels { ... }
}
```

**Optimization opportunities:**

1. **Incremental updates:** Instead of clearing the entire grid each level, only clear cells that were set in the previous iteration.

2. **Lazy clearing:** Use a generation counter instead of clearing visited bits.

### 3. `remove_duplicate_stars` (5.92%)

O(n²) algorithm comparing all star pairs:
```rust
for i in 0..stars.len() {
    for j in (i + 1)..stars.len() {
        // Distance check
    }
}
```

For images with many stars (thousands), this becomes expensive.

**Optimization opportunities:**

1. **Spatial hashing:** Use a grid/hash map to only compare nearby stars. Expected O(n) for uniform distributions.

2. **K-D tree:** Build a k-d tree for O(n log n) nearest-neighbor queries.

3. **Sort-based approach:** Sort by x-coordinate, then only compare stars within `min_separation` x-range.

### 4. `refine_centroid` + `__expf_fma` (6% + 4.5%)

Centroid refinement uses Gaussian weighting which requires `exp()` calls:
```rust
let weight = (-0.5 * dist_sq / sigma_sq).exp();
```

**Optimization opportunities:**

1. **Polynomial approximation:** Replace `exp()` with a fast polynomial approximation (loses ~0.1% accuracy).

2. **Lookup table:** Pre-compute exp values for common distance ranges.

3. **Skip refinement for faint stars:** Stars with low SNR don't benefit much from sub-pixel refinement.

### 5. Memory Allocation (~2.5%)

`malloc`, `realloc`, and page faults contribute significantly:
- Vec resizing during region collection
- Large grid allocations

**Optimization opportunities:**

1. **Pre-allocated pools:** Reuse Vec allocations across candidates.

2. **Arena allocator:** Use a bump allocator for temporary per-candidate allocations.

## Recommended Optimizations (Priority Order)

### High Impact

1. **Reduce `n_thresholds` to 32** (config change)
   - Impact: ~15-20% overall speedup
   - Risk: Low (SExtractor default)
   - Effort: Trivial

2. ~~**Spatial hashing for `remove_duplicate_stars`**~~ ✅ DONE
   - Impact: ~5% overall speedup (more for dense fields)
   - Risk: Low
   - Effort: Medium

3. **Early termination in deblending**
   - Impact: Variable (image dependent)
   - Risk: Low
   - Effort: Low

### Medium Impact

4. ~~**Fast exp approximation for centroid**~~ ✅ DONE
   - Impact: ~3-4% overall speedup (achieved **-37%** on refine_centroid)
   - Risk: Low (negligible accuracy loss)
   - Effort: Low

5. **Lazy grid clearing with generation counter**
   - Impact: ~2-3% overall speedup
   - Risk: Low
   - Effort: Medium

### Low Impact (Diminishing Returns)

6. **SIMD neighbor traversal**
   - Impact: ~2% overall speedup
   - Risk: Medium (complexity)
   - Effort: High

7. **Union-Find for connected components**
   - Impact: Uncertain
   - Risk: Medium (algorithm change)
   - Effort: High

## Quick Wins

For immediate improvement without code changes:

1. Set `n_thresholds: 32` instead of 64 in config
2. Set `n_thresholds: 0` to disable multi-threshold deblending entirely (uses simpler local-maxima approach)

## Benchmark Baseline

```
Image: rho-opiuchi.jpg (8584x5874)
Config: n_thresholds=64, min_snr=5.0, 8-connectivity
Time: 1.60s median (10 iterations)
Stars detected: [varies by config]
```

## Optimization History

### 2026-02-02: Spatial Hashing for `remove_duplicate_stars`

**Implementation:** Replaced O(n²) brute-force duplicate detection with O(n) spatial hashing.

**Approach:**
- Grid cell size = `min_separation` ensures only 3x3 neighborhood check needed
- Process stars in flux order (brightest first) to preserve priority semantics
- Fall back to O(n²) for small star counts (<100) where grid overhead dominates

**Results:**
| Stars | Before | After | Improvement |
|-------|--------|-------|-------------|
| 5,000 | 8.56ms | 5.49ms | **-35.9%** |
| 10,000 | 33.24ms | 18.62ms | **-44.0%** |

The improved scaling (only ~3.4x time for 2x stars vs 4x before) confirms the algorithm is now closer to O(n) than O(n²).

### 2026-02-02: Fast exp() Approximation for Centroid Refinement

**Implementation:** Replaced standard library `exp()` with Schraudolph fast approximation in `refine_centroid()`.

**Approach:**
- Schraudolph's method exploits IEEE 754 floating-point format
- Uses bit manipulation to compute exp(x) in ~3 integer operations
- Maximum relative error ~4% for x ∈ [-87, 0] (acceptable for Gaussian weighting)
- Monotonicity preserved (critical for centroid convergence)

**Code change:**
```rust
// Before (lumos/src/star_detection/centroid/mod.rs:458)
let weight = value * (-dist_sq / two_sigma_sq).exp();

// After
let weight = value * fast_exp(-dist_sq / two_sigma_sq);
```

**Results:**
| Benchmark | Before | After | Improvement |
|-----------|--------|-------|-------------|
| refine_centroid_single | 1.082µs | 672ns | **-37.9%** |
| refine_centroid_batch_1000 | 1.042ms | 651µs | **-37.5%** |
| compute_centroid_single (WeightedMoments) | 11.3µs | 7.2µs | **-36.4%** |

**Impact on overall detection:** ~3-4% speedup (centroid is ~6% of total time, and we improved it by ~37%).
