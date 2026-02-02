# Performance Analysis - rho-opiuchi detection (2026-02-02 v4)

**After SIMD centroid optimization:** AVX2/SSE/NEON vectorization for refine_centroid

## Benchmark Result
- **Median time:** 393ms (was 408ms in v3, 991ms in v2, ~1.6s originally)
- **Improvement:** ~4% faster vs v3, ~60% faster vs v2, ~75% faster vs original

## Top Functions by CPU Time

| Function | Self % | Notes |
|----------|--------|-------|
| `visit_neighbors_grid` | **14.02%** | BFS neighbor traversal |
| `clear_page_erms` (kernel) | 8.07% | Page zeroing for huge pages |
| `refine_centroid_avx2` | **8.06%** | SIMD centroid (was 14.78% scalar) |
| `FnMut::call_mut` (rayon) | 7.57% | Parallel iterator overhead |
| `deblend_multi_threshold` | **6.23%** | Multi-threshold deblending |
| `FnMut::call_mut` (rayon) | 4.75% | Parallel iterator overhead |
| `convolve_row_avx2` | 4.62% | Convolution (SIMD) |
| `process_words_filtered_sse` | 4.35% | Threshold mask (SIMD) |
| `find_connected_regions_grid` | **3.83%** | Connected component finding |
| `NeverShortCircuit::wrap_mut` | 3.63% | Iterator internals |
| `compute_metrics` | 3.13% | Star quality metrics |
| `interpolate_segment_avx2` | 2.45% | Background interpolation |
| `quicksort::partition` | 2.38% | Sorting operations |
| `Fn::call` (rayon) | 2.37% | Parallel iterator overhead |
| `PixelGrid::reset_with_pixels` | 2.23% | Grid reset |
| `compute_centroid` | 2.21% | Centroid computation entry point |

## Analysis

### 1. SIMD Centroid Optimization Success
- **Before (v3):** `refine_centroid` at **14.78%** (scalar)
- **After (v4):** `refine_centroid_avx2` at **8.06%** (SIMD)
- **Reduction:** 45% less CPU time in centroid refinement
- AVX2 vectorization processing 8 pixels per iteration is effective

### 2. Deblending Remains Stable
- `visit_neighbors_grid` (14.02%) + `deblend_multi_threshold` (6.23%) + `find_connected_regions_grid` (3.83%) = **24.1%**
- Consistent with v3 (24.3%), early termination optimization holding steady

### 3. Kernel Memory Operations Increased Share (8.07%)
- `clear_page_erms` rose from 5.92% to 8.07% (relatively)
- This is expected - as userspace code becomes faster, kernel overhead becomes more visible
- Absolute kernel time is likely unchanged

### 4. Rayon Overhead More Visible (~14.7%)
- Multiple `FnMut::call_mut` entries totaling ~14.7%
- As hot functions get optimized, parallel overhead becomes proportionally larger
- Could potentially benefit from work chunking adjustments

### 5. SIMD Operations Well Balanced
- `convolve_row_avx2` (4.62%) - row convolution
- `process_words_filtered_sse` (4.35%) - threshold masking
- `interpolate_segment_avx2` (2.45%) - background interpolation
- `refine_centroid_avx2` (8.06%) - centroid refinement
- All major computation paths now SIMD-optimized

## Comparison with Previous Profiles

| Function | Original | v2 | v3 | v4 | Change v3→v4 |
|----------|----------|-----|-----|-----|--------------|
| `visit_neighbors_grid` | 28.57% | 31.51% | 14.03% | 14.02% | stable |
| `refine_centroid` | ~10% | 7.82% | 14.78% | **8.06%** | **-45%** ✓ |
| `deblend_multi_threshold` | - | 19.31% | 6.37% | 6.23% | stable |
| `find_connected_regions_grid` | - | 6.82% | 3.86% | 3.83% | stable |
| `clear_page_erms` (kernel) | - | - | 5.92% | 8.07% | relative increase |
| `PixelGrid::reset_with_pixels` | 3.35% | 3.87% | 2.18% | 2.23% | stable |

## Remaining Optimization Opportunities

### Low Impact (Diminishing Returns)
1. **Rayon overhead** - ~14.7% combined in parallel iterator closures
   - Could experiment with larger work chunks or different parallelization strategies
   - May not be worth the complexity

2. **Kernel memory** - 8.07% in `clear_page_erms`
   - Page zeroing for huge pages, outside userspace control
   - Could potentially use memory pools to reduce allocations

3. **Sorting** - 2.38% in quicksort partition
   - Already using Rust's efficient unstable sort
   - Limited room for improvement

4. **compute_metrics** - 3.13%
   - Quality metrics computation
   - Could potentially SIMD-vectorize if needed

### Observation: Profile is Now Well-Balanced
No single function dominates. The top functions are:
- Graph algorithms (visit_neighbors, find_connected) - inherently sequential per component
- SIMD-optimized code (centroid, convolution, threshold, interpolation)
- Parallel runtime overhead (rayon closures)
- Kernel memory operations

This suggests we've reached a point of diminishing returns for single-function optimizations.

## Summary of Optimizations Applied

1. **Spatial hashing** for `remove_duplicate_stars` - O(n²) → O(n)
2. **Fast exp approximation** for centroid Gaussian weights
3. **Early termination** in deblending - stop after 4 levels without splits
4. **Generation counter** for PixelGrid visited array - O(n) clear → O(1)
5. **SIMD centroid refinement** - AVX2 (8 pixels), SSE4.1 (4 pixels), NEON (4 pixels)

## Performance History

| Version | Median Time | vs Previous | vs Original |
|---------|-------------|-------------|-------------|
| Original | ~1.6s | - | - |
| v2 | 991ms | -38% | -38% |
| v3 | 408ms | -59% | -75% |
| **v4** | **393ms** | **-4%** | **-75%** |

**Total speedup from original: ~4x faster (393ms vs ~1.6s)**

The SIMD centroid optimization provided a modest 4% overall improvement (15ms) because centroid refinement, while reduced from 14.78% to 8.06% of CPU time, was already a smaller portion of the total workload after previous optimizations.
