# Performance Analysis - rho-opiuchi detection (2026-02-05)

**Centroid method:** `MoffatFit { beta: 2.5 }` (L-M optimizer with Moffat profile).
**Image:** 8584x5874 (rho-opiuchi.jpg), `Config::precise_ground()`

## Version History

| Version | Centroid Method | Median Time | Top Bottleneck |
|---------|----------------|-------------|----------------|
| v2 | WeightedMoments | 991ms | visit_neighbors (31%) |
| v3 | WeightedMoments | 408ms | visit_neighbors (14%) |
| v4 | WeightedMoments + SIMD | 393ms | visit_neighbors (14%) |
| v5 | MoffatFit f64 (pre-opt) | 726ms | libm::pow (36%) |
| v5a | MoffatFit f64 (fused+fast_pow) | 523ms | lm_optimizer (55%) |
| **v5b** | **MoffatFit f64 + AVX2 SIMD** | **435ms** | **background (12.5%)** |

v5+ use L-M Moffat fitting (f64, ~0.01px accuracy) instead of WeightedMoments (~0.05px).

---

## Current Profile (v5b) — After AVX2 SIMD Vectorization

### Perf Configuration
- Sample rate: 3000 Hz, DWARF call graphs
- 358K samples collected across 10 benchmark iterations
- Event: `cpu_atom/cycles/P`

### Top Functions by CPU Time (self %)

| Function | Self % | Notes |
|----------|--------|-------|
| `simd_avx2::batch_fill_jacobian_residuals_avx2` | **15.14%** | AVX2 Jacobian+residuals (was `optimize` at 21.7%) |
| `background::interpolate_segment_avx2` | **12.51%** | Background interpolation |
| `simd_avx2::batch_compute_chi2_avx2` | **9.11%** | AVX2 chi² for trial steps |
| `deblend::bfs_region` | 8.26% | BFS neighbor traversal |
| `deblend::deblend_multi_threshold` | 7.62% | Multi-threshold deblending |
| `threshold_mask::process_words_sse` | 6.77% | Threshold masking |
| `lm_optimizer::optimize` | 5.53% | L-M loop overhead (Hessian, solve, etc.) |
| `threshold_mask::process_words_filtered_sse` | 2.07% | Filtered threshold |
| `rayon::bridge_producer_consumer` | 2.09% | Rayon parallelism overhead |
| `quicksort::partition` | 1.73% | Sorting |
| `convolution::convolve_row_avx2` | 1.54% | Row convolution |
| `centroid::refine_centroid` | 1.46% | Weighted moments seed (2 iters) |
| `libm::exp` | ~1.5% | From `refine_centroid` Gaussian weights |
| `centroid::compute_metrics` | 1.08% | Star quality metrics |
| `deblend::PixelGrid::reset_with_pixels` | 1.04% | Grid initialization |
| `linear_solver::solve` | 0.95% | Gaussian elimination (5x5) |
| `centroid::measure_star` | 0.78% | Measurement entry point |
| `Vec::extend_desugared` | 0.61% | Vec push in SIMD tail / jacobian storage |

### Grouped by Pipeline Stage

| Stage | % CPU | Notes |
|-------|-------|-------|
| **Centroid fitting (SIMD + optimizer)** | **~33%** | batch_fill_jac (15%) + batch_chi2 (9%) + optimize (5.5%) + solve (1%) + misc (2.5%) |
| **Deblending** | **~17%** | bfs_region (8.3%) + deblend_multi_threshold (7.6%) + PixelGrid (1%) |
| **Background** | **~12.5%** | interpolate_segment_avx2 |
| **Threshold mask** | **~9%** | process_words_sse (6.8%) + filtered_sse (2.1%) |
| Rayon overhead | ~2.1% | bridge_producer_consumer |
| Sorting | ~1.7% | quicksort::partition |
| Convolution | ~1.5% | convolve_row_avx2 |
| Centroid (non-fitting) | ~2.5% | refine_centroid (1.5%) + compute_metrics (1.1%) |
| libm (exp only) | ~1.5% | Gaussian exp() in refine_centroid |

### Key Change: `libm::pow` eliminated

The most significant result: **`libm::pow` is completely gone from the profile.**

| | v5 (pre-opt) | v5b (current) |
|---|---|---|
| `libm::pow` | **~36%** of CPU | **0%** |
| Centroid fitting total | ~63% | ~33% |
| Background (relative) | 6% | 12.5% |
| Deblending (relative) | 10% | 17% |

The elimination of `libm::pow` shifted the profile from being dominated by a
single expensive function to a more balanced distribution across pipeline stages.

---

## Optimizations Applied

### v5a: Algorithmic Optimizations (726ms -> 523ms, -28%)

1. **Fused evaluate+jacobian** — `LMModel::evaluate_and_jacobian()` computes
   model value and Jacobian row in a single pass, sharing intermediate values
   (`u`, `u_neg_beta`). Reduces powf calls from 2 to 1 per pixel in
   `fill_jacobian_residuals`.

2. **Chi2 from residuals** — `fill_jacobian_residuals` returns chi2 computed
   from already-calculated residuals. On first iteration this replaces a
   separate `compute_chi2` call.

3. **Fast powf for fixed beta** — `PowStrategy` enum pre-selects optimal method:
   - Half-integer beta (2.5, 3.5): `1 / (u^n * sqrt(u))` — sqrt + int_pow
   - Integer beta (2, 3, 4): `1 / u^n` — int_pow only
   - General: fallback to `u.powf(-beta)`

### v5b: AVX2 SIMD Vectorization (523ms -> 435ms, -17%)

4. **AVX2+FMA batch operations** — `MoffatFixedBeta` overrides
   `batch_fill_jacobian_residuals` and `batch_compute_chi2` with AVX2+FMA
   implementations processing 4 f64 pixels per `__m256d` register:
   - `_mm256_sqrt_pd` for sqrt(u) in HalfInt strategy
   - `_mm256_fmadd_pd` for fused multiply-add (r2, u, model_val, chi2)
   - `simd_int_pow` for integer powers via repeated squaring in SIMD lanes
   - `simd_fast_pow_neg` dispatches per PowStrategy (HalfInt/Int/General)
   - Scalar tail handles remainder pixels (n % 4)
   - Runtime dispatch via `cpu_features::has_avx2_fma()` with scalar fallback

#### Microbenchmark Results

| Benchmark | v5a | v5b | Change |
|-----------|-----|-----|--------|
| fixed_beta_small (17x17) | 20.6us | 12.5us | **-39.5%** |
| fixed_beta_medium (25x25) | 43.4us | 26.3us | **-39.5%** |
| moffat_fit_single (21x21) | 20.7us | 12.5us | **-39.3%** |
| measure_star_moffat_fit | 11.4us | 8.6us | **-24.9%** |
| batch_6k_10000/moffat_fit | 59.3ms | 47.5ms | **-19.9%** |
| variable_beta (no SIMD) | 68.4us | 67.9us | -0.7% (unchanged) |

#### End-to-End Result

| Metric | v5 (pre-opt) | v5a | v5b | Total Change |
|--------|-------------|-----|-----|-------------|
| Median | 726ms | 523ms | **435ms** | **-40.1%** |

---

## Remaining Optimization Opportunities

### Medium Impact
- **Better initial parameter estimates** — reduce L-M iterations via linear
  least-squares seed for amplitude/background
- **SIMD vectorize `compute_hessian_gradient`** — J^T J accumulation is 5.5%
  of CPU within `optimize`, could benefit from SIMD outer product accumulation

### Low Impact
- **SIMD for `MoffatVariableBeta`** — would need SIMD `ln`/`exp` approximation,
  variable beta is less common in production
- **Background interpolation** — already AVX2 vectorized at 12.5%, diminishing returns
- **Deblending** — graph algorithms at 17%, inherently sequential per component,
  already optimized with early termination and generation counters
