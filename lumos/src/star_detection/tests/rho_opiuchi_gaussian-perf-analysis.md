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
| `centroid::compute_metrics` | 0.68% | - | Star quality metrics |

### Grouped by Pipeline Stage (pre-optimization)

| Stage | % CPU | Notes |
|-------|-------|-------|
| **Centroid fitting (L-M + exp)** | **~51%** | batch_build (22%) + exp (21%) + optimize (3.4%) + chi2 via exp (~3%) + solve (0.8%) |
| **Background** | **~8.7%** | interpolate_segment_avx2 |
| **Deblending** | **~11%** | bfs_region (5.3%) + deblend_multi_threshold (5.1%) + PixelGrid (0.7%) |
| **Threshold mask** | **~7%** | process_words_sse (5.9%) + filtered_sse (1.3%) |
| Convolution | ~1.5% | convolve_row_avx2 |
| Centroid (non-fitting) | ~3% | refine_centroid (0.9%) + compute_metrics (0.7%) + measure_star |
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
