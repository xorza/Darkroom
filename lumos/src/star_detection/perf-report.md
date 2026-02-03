# Star Detection Performance Report

**Benchmark:** `quick_bench_detect_rho_opiuchi` (`precise_ground()` + `MoffatFit { beta: 2.5 }` centroid)
**Image:** rho-opiuchi.jpg, 8584x5874 pixels, grayscale
**Runtime:** 502ms median (release, 10 iters)
**Samples:** 203,856 (perf record -F 3000, --call-graph dwarf, --no-inline)
**Date:** 2026-02-03
**Last updated:** 2026-02-03 (switched from GaussianFit to MoffatFit)

## Top Functions (self time)

| %      | Function | Module |
|--------|----------|--------|
| 13.35% | `optimize_moffat_fixed_beta_avx2` | moffat_fit (L-M optimizer) |
| 11.72% | `bfs_region` | deblend |
| 8.58%  | `deblend_multi_threshold` | deblend |
| 6.00%  | `compute_pow_neg_beta_8` | moffat_fit (SIMD pow) |
| 5.64%  | closure (FnMut) | background interpolation |
| 5.15%  | `fill_jacobian_residuals_simd_fixed_beta` | moffat_fit (SIMD) |
| 4.07%  | iterator fold (map) | various |
| 3.27%  | `convolve_row_avx2` | convolution |
| 3.16%  | `process_words_filtered_sse` | threshold_mask |
| 2.67%  | `refine_centroid_avx2` | centroid |
| 2.55%  | `compute_metrics` | centroid |
| 2.52%  | `NeverShortCircuit` closure | iterator internals |
| 2.40%  | `clear_page_erms` (kernel) | page faults |
| 2.34%  | `compute_chi2_simd_fixed_beta` | moffat_fit (SIMD) |
| 2.01%  | `quicksort::partition` | sorting |
| 1.84%  | `interpolate_segment_avx2` | background |
| 1.81%  | `compute_centroid` | centroid |
| 1.76%  | closure (Fn) | various |
| 1.56%  | `__memmove_avx_unaligned_erms` | libc (memcpy) |
| 1.55%  | `PixelGrid::reset_with_pixels` | deblend |
| 1.00%  | `find_connected_regions_grid` | deblend |
| 0.93%  | `roundf` | libm |
| 0.91%  | `from_elem` | allocator (vec init) |
| 0.89%  | `convolve_cols_avx2` | convolution |
| 0.88%  | `solve_generic` | moffat_fit (linear solver) |
| 0.86%  | `extend_desugared` | allocator (vec extend) |
| 0.65%  | `extract_stamp` | centroid |
| 0.65%  | `sigma_clip_iteration` | background |

## Grouped by Pipeline Stage

| Stage | % of total | Key functions |
|-------|-----------|---------------|
| **Moffat fitting** | ~28% | `optimize_moffat_fixed_beta_avx2` (13.4%), `compute_pow_neg_beta_8` (6.0%), `fill_jacobian_residuals_simd_fixed_beta` (5.2%), `compute_chi2_simd_fixed_beta` (2.3%), `solve_generic` (0.9%) |
| **Deblending** | ~23% | `bfs_region` (11.7%), `deblend_multi_threshold` (8.6%), `reset_with_pixels` (1.6%), `find_connected_regions_grid` (1.0%) |
| **Centroid + metrics** | ~7% | `refine_centroid_avx2` (2.7%), `compute_metrics` (2.6%), `compute_centroid` (1.8%) |
| **Background** | ~8% | closures (5.6%), `interpolate_segment_avx2` (1.8%), `sigma_clip_iteration` (0.7%) |
| **Convolution** | ~7% | `convolve_row_avx2` (3.3%), `clear_page_erms` (2.4%), `convolve_cols_avx2` (0.9%) |
| **Threshold mask** | ~3% | `process_words_filtered_sse` (3.2%) |
| **Other** | ~24% | iterator overhead (8.4%), sorting (2.0%), memcpy (1.6%), allocator (1.8%), rayon scheduling |

## Change History

### Optimization 5: Convolution buffer pooling (492ms → 478ms, -2.8%)

The separable Gaussian convolution in `gaussian_convolve()` allocated a fresh ~200MB temp buffer (`Buffer2::new_default(width, height)`) on every call. For the 8584x5874 image, this caused ~50K page faults per `detect()` call, with `clear_page_erms` consuming 6.3% of pipeline time.

**Fix:** Thread a `&mut Buffer2<f32>` temp buffer parameter through the convolution call chain (`matched_filter` → `elliptical_gaussian_convolve` → `gaussian_convolve`). The detector acquires it from `BufferPool`, so on repeated `detect()` calls the buffer is reused with no page faults.

**Results:**
- Full pipeline: 492ms → 478ms (**-2.8%**)
- First `detect()` call: unchanged (pool allocates fresh buffer)
- Subsequent calls: ~14ms savings (page faults eliminated for convolution temp)

### Optimization 7: Early L-M termination on position convergence (462ms → 445ms, -3.8%)

The L-M optimizer converges all 6 parameters (x0, y0, amplitude, sigma_x, sigma_y, background) to 1e-6 threshold. For centroid computation, only position (x0, y0) matters — continuing iterations to refine sigma/amplitude wastes cycles.

**Fix:** Added `position_convergence_threshold` to `LMConfig`. When set, the optimizer terminates early once both position deltas are below the threshold (0.001px), even if other parameters are still changing. Applied to both the generic L-M optimizer and the SIMD-optimized Gaussian/Moffat paths.

**Results:**
- GaussianFit single: 8.356µs → 6.041µs (**-27.7%**)
- MoffatFit single: 8.697µs → 8.677µs (unchanged — chi2 stagnation was already triggering early exit)
- Full pipeline: 462ms → 445ms (**-3.8%**)

### Optimization 6: Reduce Phase 1 iterations for fitting methods (478ms → 462ms, -2.5%)

`compute_centroid` runs Phase 1 (iterative weighted moments, up to 10 iterations) before Phase 2 (L-M fitting). When fitting follows, Phase 1 only needs to provide a reasonable seed — the L-M optimizer refines position independently and converges to the same result regardless of Phase 1 precision (verified by tests).

**Fix:** Reduced Phase 1 from 10 to 2 iterations when `CentroidMethod` is `GaussianFit` or `MoffatFit`. `WeightedMoments` mode still uses 10 iterations.

**Results:**
- GaussianFit single: 10.24µs → 8.356µs (**-18.4%**)
- MoffatFit single: 13.586µs → 8.697µs (**-36.0%**)
- Full pipeline: 478ms → 462ms (**-2.5%**)

### Optimization 4: Deblend allocation reduction (503ms → 495ms, -1.6%)

Reduced per-component allocations in multi-threshold deblending:
1. **NodeGrid generation counter** — eliminated O(n) clearing per `reset_with_pixels` (same pattern as PixelGrid values optimization from Opt 3).
2. **Region Vec pooling** — recycled `Vec<Pixel>` allocations across BFS calls via a `region_pool`. Region vecs are drained back to the pool after each threshold level.
3. **Per-call `DeblendBuffers`** — consolidated all reusable buffers (`component_pixels`, `pixel_to_node`, `above_threshold`, `parent_pixels_above`, `bfs_queue`, `regions`, `region_pool`, `pixel_grid`) into a single struct created once per `deblend_multi_threshold` call. All inner-loop allocations reuse these buffers.
4. **Rayon fold/reduce buffer reuse** — in `extract_candidates`, each rayon thread gets its own `DeblendBuffers` via `fold` identity, reused across all components on that thread.

**Results:**
- `bench_multi_threshold_4k_dense`: 73.7ms → 69.7ms (**-5.2%**)
- Full pipeline: 501ms → 495ms (**-1.3%**)

### Optimization 3: Deblend BFS optimization (510ms → 503ms, ~-1%)

Deblending was 21% of pipeline time. Optimized `PixelGrid` and connected component BFS:
1. Generation counter for values — eliminated O(n) `clear()+resize()` memset per `reset_with_pixels` call.
2. Flat-index BFS queue — `Vec<u32>` (4B) instead of `Vec<Pixel>` (24B), reducing memory traffic. No coordinate conversions in the hot loop.
3. Unchecked neighbor access — fixed border guarantee at coordinate 0 (`wrapping_sub` vs `saturating_sub`), enabling fully unchecked 8-neighbor access.

**Results:**
- `bench_multi_threshold_4k_dense`: 76.3ms → 73.3ms (**-3.9%**)
- Full pipeline: 506ms → 503ms (within noise, deblending is memory-bound and BFS-dominated)

### Optimization 2: AVX2 `compute_hessian_gradient_6` (560ms → 510ms, -8.3%)

The #1 hotspot at 16.0% of total time. Scalar triple-nested loop computing J^T·J (Hessian) and J^T·r (gradient) with 36 multiply-accumulates + 6 gradient updates per pixel.

**Implementation:** AVX2 outer-product accumulation processing 8 Jacobian rows at a time with 21 upper-triangle FMA accumulators + 6 gradient accumulators. Manual `_mm256_setr_ps` loads from stride-6 AoS layout (gather instructions were tried first but only gave -4.8% due to 7-12 cycle gather latency). Scalar tail handles remaining rows. Upper triangle mirrored to lower.

**Results:**
- `bench_gaussian_fit_single`: 12.9µs → 9.0µs (**-30.3%**)
- Full pipeline: 560ms → 510ms (**-8.3%**)
- Accuracy: identical (bitwise, same operations in different order)

### Optimization 1: AVX2 `fast_exp` (639ms → 560ms, -12.4%)

Previous profile: 639ms, `__expf_fma` at 9.67% (scalar libm exp called from SIMD Jacobian/chi2).

1. **`__expf_fma` eliminated (9.7% → 0%).** Fast polynomial SIMD exp (`fast_exp_8_avx2_m256`) now runs entirely inside `fill_jacobian_residuals_avx2` and `compute_chi2_avx2`. No scalar libm calls remain in the profile.
2. **Runtime dropped 639ms → 560ms (-12.4%).** The fast_exp change accounts for most of this.
3. **`n_thresholds` is 32 (SExtractor default), not 64 as previously reported.

## Optimization Opportunities

### ~~1. SIMD `compute_hessian_gradient_6`~~ — Done (-8.3%)

Implemented AVX2 outer-product accumulation with manual loads (gather was too slow). See Change History above for details.

### ~~2. Reduce deblending cost~~ — Done (-8.7% deblend, -2.6% pipeline cumulative)

Two rounds of optimization to deblending:

**Round 1 — BFS optimization (Opt 3):**
- Generation counter for PixelGrid values (O(1) reset vs O(n) memset)
- Flat-index BFS queue (`Vec<u32>` vs `Vec<Pixel>`)
- Unchecked 8-neighbor access with `wrapping_sub` border guarantee
- Results: 76.3ms → 73.3ms (-3.9% deblend)

**Round 2 — Allocation reduction (Opt 4):**
- NodeGrid generation counter (same pattern)
- Region Vec pooling across BFS calls
- Per-call `DeblendBuffers` consolidating all reusable buffers
- Rayon fold/reduce for cross-thread buffer reuse
- Results: 73.7ms → 69.7ms (-5.2% deblend)

**Cumulative deblend results:** 76.3ms → 69.7ms (**-8.7%**)
**Cumulative pipeline results:** 506ms → 495ms (**-2.2%**)

### ~~3. Pool convolution temp buffer~~ — Done (-2.8%)

Threaded a `&mut Buffer2<f32>` temp buffer through the convolution call chain and acquired it from `BufferPool` in the detector. On repeated `detect()` calls, the buffer is reused — no page faults for the ~200MB separable convolution intermediate. See Optimization 5 in Change History.

### ~~4. Reduce centroid refinement passes~~ — Done (-2.5%)

Reduced Phase 1 from 10 to 2 iterations when fitting follows. L-M converges to same result regardless. See Optimization 6 in Change History.

### ~~5. Early L-M termination on position convergence~~ — Done (-3.8%)

Added `position_convergence_threshold` to `LMConfig` for early exit when position is stable. See Optimization 7 in Change History.

### 6. Deblending remains the #1 stage (24%)

Despite two rounds of optimization, deblending is still the largest pipeline stage. `bfs_region` alone is 12.4%. The BFS is memory-bound (random access pattern over pixel grid). Further gains would require algorithmic changes:
- Reduce `n_thresholds` from 32 to 16 (halves BFS work, may reduce deblending quality).
- Skip deblending for isolated components (no neighbors within min_separation).
- Spatial locality improvements (sort pixels by Morton/Hilbert curve before BFS).

## Summary

| Optimization | Effort | Impact | Accuracy risk | Status |
|-------------|--------|--------|---------------|--------|
| ~~Vectorized AVX2 `expf`~~ | Medium | **-12.4%** (639→560ms) | None (<1e-6) | **Done** |
| ~~AVX2 hessian/gradient~~ | Medium | **-8.3%** (560→510ms) | None | **Done** |
| ~~Deblend BFS optimization~~ | Low | **-3.9%** deblend, ~-1% pipeline | None | **Done** |
| ~~Deblend allocation reduction~~ | Low | **-5.2%** deblend, -1.3% pipeline | None | **Done** |
| ~~Pool convolution temp buffer~~ | Low | **-2.8%** (492→478ms) | None | **Done** |
| ~~Reduce centroid Phase 1 iters~~ | Low | **-2.5%** (478→462ms) | None | **Done** |
| ~~Early L-M position termination~~ | Low | **-3.8%** (462→445ms) | None | **Done** |
| **Cumulative done (GaussianFit)** | | **-30.4%** (639→445ms) | | |
| **Current (MoffatFit)** | | **502ms** median | | |
