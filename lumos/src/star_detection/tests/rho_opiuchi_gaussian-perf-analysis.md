# Performance Analysis - rho-opiuchi detection (GaussianFit)

**Centroid method:** `GaussianFit` (L-M optimizer with 2D Gaussian profile, 6 params).
**Image:** 8584x5874 (rho-opiuchi.jpg), `Config::precise_ground()`

## Benchmark Result

| Centroid Method | Median Time | Params | SIMD |
|----------------|-------------|--------|------|
| MoffatFit (v5c) | **435ms** | 5 (fixed beta) | AVX2 fused normal equations |
| **GaussianFit** | **587ms** | 6 | Scalar (default trait impl) |

GaussianFit is **35% slower** than optimized MoffatFit. The difference is entirely
due to Gaussian using the scalar default `batch_build_normal_equations` (no AVX2
override) and `libm::exp` being expensive.

## Perf Configuration

- Sample rate: 3000 Hz, DWARF call graphs
- 535K samples collected across 10 benchmark iterations
- Event: `cpu_atom/cycles/P`

## Top Functions by CPU Time (self %)

| Function | Self % | Inclusive % | Notes |
|----------|--------|-------------|-------|
| `LMModel::batch_build_normal_equations` | **22.39%** | **37.80%** | Default scalar impl, calls exp() |
| `background::interpolate_segment_avx2` | 8.37% | 8.69% | Background interpolation |
| `libm::exp` (all symbols) | **~21%** | **25.91%** | Gaussian exp(-0.5*r²/σ²) |
| `threshold_mask::process_words_sse` | 5.87% | 5.93% | Threshold masking |
| `deblend::bfs_region` | 5.34% | 5.37% | BFS neighbor traversal |
| `deblend::deblend_multi_threshold` | 5.11% | 5.86% | Multi-threshold deblending |
| `lm_optimizer::optimize` | 3.40% | 12.89% | L-M loop (calls batch_compute_chi2 → exp) |
| `convolution::convolve_row_avx2` | 1.47% | 1.59% | Row convolution |
| `threshold_mask::process_words_filtered_sse` | 1.25% | - | Filtered threshold |
| `quicksort::partition` | 1.14% | - | Sorting |
| `centroid::refine_centroid` | 0.92% | 2.02% | Weighted moments seed |
| `linear_solver::solve` | 0.81% | - | Gaussian elimination (6x6) |
| `centroid::compute_metrics` | 0.68% | - | Star quality metrics |

## Grouped by Pipeline Stage

| Stage | % CPU | Notes |
|-------|-------|-------|
| **Centroid fitting (L-M + exp)** | **~51%** | batch_build (22%) + exp (21%) + optimize (3.4%) + chi2 via exp (~3%) + solve (0.8%) |
| **Background** | **~8.7%** | interpolate_segment_avx2 |
| **Deblending** | **~11%** | bfs_region (5.3%) + deblend_multi_threshold (5.1%) + PixelGrid (0.7%) |
| **Threshold mask** | **~7%** | process_words_sse (5.9%) + filtered_sse (1.3%) |
| Convolution | ~1.5% | convolve_row_avx2 |
| Centroid (non-fitting) | ~3% | refine_centroid (0.9%) + compute_metrics (0.7%) + measure_star |
| Rayon/sorting | ~3% | rayon overhead + quicksort |

## Key Observation: `exp` Dominates

The Gaussian model uses `(-0.5 * r² / σ²).exp()` for every pixel evaluation.
Unlike Moffat's `u^(-beta)` which can be replaced with `int_pow + sqrt` for
half-integer beta, there is no algebraic shortcut for `exp()`.

| | MoffatFit (v5c) | GaussianFit |
|---|---|---|
| `libm::exp` | **0%** (eliminated via fast_pow) | **~21%** of CPU |
| `libm::pow` | **0%** | 0% |
| Fitting total | ~15% | ~51% |
| Non-fitting stages | ~85% | ~49% |

The Gaussian profile spends **half its time** in L-M fitting, with `exp()` being
the single largest cost. The Moffat profile avoids this entirely for common
beta values (2.0, 2.5, 3.0, 3.5) via `PowStrategy`.

## Optimization Opportunities

### High Impact
- **AVX2 SIMD for Gaussian `batch_build_normal_equations`** — same fused
  21-accumulator approach as MoffatFixedBeta, but for N=6 (28 upper-triangle
  hessian + 6 gradient + 1 chi² = 35 accumulators). Fits in 32 YMM registers
  if we exploit j5=1.0 (background). Would eliminate per-pixel function call
  overhead and enable FMA throughout.
- **Fast `exp` approximation** — SIMD `exp` via polynomial approximation
  (e.g., Cephes-style or Remez minimax). Could use AVX2 `_mm256_castpd_si256`
  for bit manipulation. Accuracy of ~1e-12 is sufficient for L-M fitting.
  Combined with SIMD batch, this would eliminate the ~21% `libm::exp` cost.

### Medium Impact
- **Reduce L-M iterations** — better initial parameter estimates via linear
  least-squares seed for amplitude/background.

### Low Impact
- **Switch to MoffatFit** — already 35% faster with SIMD, and Moffat models
  atmospheric PSF wings better than Gaussian. GaussianFit is mainly useful
  for space-based or well-sampled diffraction-limited PSFs.
