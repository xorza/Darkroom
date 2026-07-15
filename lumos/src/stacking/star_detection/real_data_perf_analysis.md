# Performance Analysis - rho-opiuchi detection (GaussianFit)

**Centroid method:** `GaussianFit` (L-M optimizer with 2D Gaussian profile, 6 params).
**Image:** 8584x5874 (rho-opiuchi.jpg), `Config::precise_ground()`

## Optimization Results

### Microbenchmark Summary (10K iterations)

| Version | Small (17x17) | Medium (25x25) | Large (31x31) |
|---------|--------------|----------------|----------------|
| Scalar baseline | 25.8µs | 52.9µs | 83.1µs |
| + AVX2 SIMD (scalar exp) | 13.8µs (-47%) | 28.2µs (-47%) | 43.5µs (-48%) |
| + Fast SIMD exp (Cephes) | 8.3µs (-40%) | 16.2µs (-43%) | 24.2µs (-44%) |
| **Total improvement** | **-68%** | **-69%** | **-71%** |

### Optimizations Applied

1. **AVX2 SIMD fused normal equations** (`simd_avx2.rs`):
   - Processes 4 f64 pixels per iteration using `__m256d`
   - 28 AVX2 accumulator registers: 21 upper-triangle Hessian + 6 gradient + 1 chi²
   - Exploits j5=1.0 (background Jacobian) to use `_mm256_add_pd` instead of FMA
   - Both `batch_build_normal_equations` and `batch_compute_chi2` overridden
   - Runtime dispatch via `common::cpu_features::has_avx2_fma()` with scalar fallback

2. **Fast vectorized exp() approximation** (`simd_exp_fast`):
   - Cephes-derived rational polynomial approximation (P/Q degree 2/3)
   - Range reduction: x = n*ln(2) + r, |r| ≤ ln(2)/2
   - Two-part ln(2) (hi/lo) for precision in range reduction
   - Reconstruction via IEEE 754 bit manipulation: `2^n * exp(r)`
   - ~1e-13 relative accuracy (verified by tests across [-700, 700])
   - Eliminates all `libm::exp` function call overhead

## Pre-optimization Profiling

### Perf Configuration

- Sample rate: 3000 Hz, DWARF call graphs
- 535K samples collected across 10 benchmark iterations
- Event: `cpu_atom/cycles/P`

### Top Functions by CPU Time (self %)

| Function | Self % | Inclusive % | Notes |
|----------|--------|-------------|-------|
| `LMModel::batch_build_normal_equations` | **22.39%** | **37.80%** | Default scalar impl, calls exp() |
| `background::interpolate_segment_avx2` | 8.37% | 8.69% | Background interpolation |
| `libm::exp` (all symbols) | **~21%** | **25.91%** | Gaussian exp(-0.5*r²/σ²) |
| `threshold_mask::process_words_sse` | 5.87% | 5.93% | Threshold masking |
| `deblend::bfs_region` | 5.34% | 5.37% | BFS neighbor traversal |
| `deblend::deblend_multi_threshold` | 5.11% | 5.86% | Multi-threshold deblending |
| `lm_optimizer::optimize` | 3.40% | 12.89% | L-M loop (calls batch_compute_chi2 -> exp) |
| `convolution::convolve_row_avx2` | 1.47% | 1.59% | Row convolution |
| `threshold_mask::process_words_filtered_sse` | 1.25% | - | Filtered threshold |
| `quicksort::partition` | 1.14% | - | Sorting |
| `centroid::refine_centroid` | 0.92% | 2.02% | Weighted moments seed |
| `linear_solver::solve` | 0.81% | - | Gaussian elimination (6x6) |
| `centroid::compute_star` | 0.68% | - | Star quality metrics |

### Grouped by Pipeline Stage (pre-optimization)

| Stage | % CPU | Notes |
|-------|-------|-------|
| **Centroid fitting (L-M + exp)** | **~51%** | batch_build (22%) + exp (21%) + optimize (3.4%) + chi2 via exp (~3%) + solve (0.8%) |
| **Background** | **~8.7%** | interpolate_segment_avx2 |
| **Deblending** | **~11%** | bfs_region (5.3%) + deblend_multi_threshold (5.1%) + PixelGrid (0.7%) |
| **Threshold mask** | **~7%** | process_words_sse (5.9%) + filtered_sse (1.3%) |
| Convolution | ~1.5% | convolve_row_avx2 |
| Centroid (non-fitting) | ~3% | refine_centroid (0.9%) + compute_star (0.7%) + measure_star |
| Rayon/sorting | ~3% | rayon overhead + quicksort |

### Key Observation: `exp` Dominated (pre-optimization)

The Gaussian model uses `(-0.5 * r² / σ²).exp()` for every pixel evaluation.
Unlike Moffat's `u^(-beta)` which can be replaced with `int_pow + sqrt` for
half-integer beta, there is no algebraic shortcut for `exp()`.

| | MoffatFit (v5c) | GaussianFit (pre-opt) | GaussianFit (optimized) |
|---|---|---|---|
| `libm::exp` | **0%** | **~21%** of CPU | **0%** (replaced by SIMD polynomial) |
| Fitting total | ~15% | ~51% | ~15-20% (estimated) |

## Remaining Optimization Opportunities

### Medium Impact
- **Reduce L-M iterations** -- better initial parameter estimates via linear
  least-squares seed for amplitude/background.

### Low Impact
- **Switch to MoffatFit** -- already fast with SIMD, and Moffat models
  atmospheric PSF wings better than Gaussian. GaussianFit is mainly useful
  for space-based or well-sampled diffraction-limited PSFs.

---

## ARM64 (Apple Silicon) Profile - 2026-02-06

**Platform:** macOS, ARM64E (Apple M-series)
**Profiler:** macOS `sample` tool, 1ms sampling interval, 30s duration
**Benchmark:** `quick_bench_detect_rho_opiuchi` (10 iters, 1 warmup)

### Timing

| Metric | Value |
|--------|-------|
| Median | **1.30s** |
| Min | 1.30s |
| Max | 1.33s |

### Sample Breakdown

| Category | Samples | % of Total |
|----------|---------|-----------|
| Active (on-CPU) | 32,771 | 71.1% |
| Idle (thread waits) | 13,291 | 28.9% |
| **Total** | **46,062** | 100% |

### Top Functions by Exclusive CPU Time

| Function | Samples | Self % | Notes |
|----------|---------|--------|-------|
| `Gaussian2D::batch_build_normal_equations` | 10,247 | **31.3%** | L-M Hessian+gradient (scalar on ARM) |
| `exp` (libsystem_m) | 7,678 | **23.4%** | Called from Gaussian model |
| `DYLD-STUB$$exp` | 840 | 2.6% | Dynamic linker stub for exp |
| `deblend::bfs_region` | 2,702 | **8.2%** | BFS flood-fill in deblending |
| `deblend::deblend_multi_threshold` | 1,961 | **6.0%** | Multi-threshold deblend loop |
| `gaussian_fit::fit_gaussian_2d` | 1,139 | 3.5% | L-M outer loop |
| `convolution::neon::convolve_cols_neon` | 456 | 1.4% | Column convolution (NEON) |
| `centroid::compute_star` | 430 | 1.3% | Star quality metrics |
| `centroid::linear_solver::solve` | 427 | 1.3% | 6x6 Gaussian elimination |
| `quicksort::partition` (combined) | 663 | 2.0% | Sorting (multiple monomorphizations) |
| `centroid::measure_star` | 274 | 0.8% | Per-star measurement dispatch |
| `convolution::neon::convolve_row_neon` | 262 | 0.8% | Row convolution (NEON) |
| `PixelGrid::reset_with_pixels` | 233 | 0.7% | Deblend grid setup |
| `memmove` | 458 | 1.4% | Memory copies |
| `sigma_clip_iteration` | 163 | 0.5% | Statistics |
| `centroid::refine_centroid` | 157 | 0.5% | Weighted moments seed |

### Grouped by Pipeline Stage

| Stage | Self % | Notes |
|-------|--------|-------|
| **Centroid fitting (L-M + exp)** | **~63%** | batch_build (31.3%) + exp (23.4%+2.6%) + fit_gaussian (3.5%) + solve (1.3%) + measure_star (0.8%) |
| **Deblending** | **~15%** | bfs_region (8.2%) + deblend_multi_threshold (6.0%) + PixelGrid (0.7%) |
| **Convolution** | **~2.2%** | convolve_cols_neon (1.4%) + convolve_row_neon (0.8%) |
| **Centroid (non-fitting)** | **~2.3%** | compute_star (1.3%) + refine_centroid (0.5%) + extract_stamp (0.2%) |
| **Sorting** | ~2.0% | quicksort partitions |
| **Memory ops** | ~1.4% | memmove |
| **Statistics** | ~0.5% | sigma_clip_iteration |

### Key Observations (ARM64 vs x86 AVX2)

1. **exp() is back to dominating**: On x86, the Cephes SIMD `exp()` approximation
   eliminated `libm::exp` overhead entirely. On ARM64, there is no equivalent NEON
   fast-exp implementation, so `exp()` consumes **26%** of CPU (vs 0% on x86 post-opt).

2. **batch_build_normal_equations is scalar**: The AVX2 SIMD path doesn't apply on ARM.
   The scalar implementation runs at 31.3% — similar to the pre-optimization x86 profile
   (22.4%), but proportionally higher because background/threshold stages are faster on ARM.

3. **Deblending is proportionally larger**: At 15% (vs 11% on x86), deblending is now
   the second largest stage. `bfs_region` (8.2%) suggests the flood-fill BFS traversal
   may benefit from optimization on ARM.

4. **Convolution is efficient on NEON**: Only 2.2% total — the NEON convolution
   implementation is performing well.

5. **Background/threshold not visible**: These stages don't appear in the top functions,
   suggesting they are very fast on ARM (likely <1% each).

### Optimization Opportunities (ARM64)

#### High Impact
- **NEON SIMD for `batch_build_normal_equations`**: Port the AVX2 implementation
  to ARM NEON using `float64x2_t` (2-wide f64). Expected ~2x speedup for the 31.3%
  hotspot, saving ~15% total time.

- **NEON fast exp() approximation**: Port the Cephes polynomial exp approximation
  to NEON `float64x2_t`. Would eliminate the 26% `exp()` overhead. Combined with
  NEON normal equations, this could reduce fitting from 63% to ~20%.

#### Medium Impact
- **Deblend BFS optimization**: The `bfs_region` (8.2%) uses a queue-based BFS.
  Consider block-based flood-fill or bitset-based visited tracking to improve
  cache behavior on ARM's smaller L1.
