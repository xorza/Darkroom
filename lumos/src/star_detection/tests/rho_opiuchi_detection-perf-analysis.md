# Performance Analysis - rho-opiuchi detection (2026-02-05 v5)

**After SIMD removal, f64 migration, and LMModel trait unification.**
Centroid method: `MoffatFit { beta: 2.5 }` (L-M optimizer with Moffat profile).

## Benchmark Result
- **Median time:** 750ms (was 393ms in v4 with WeightedMoments centroid)
- **Image:** 8584x5874 (rho-opiuchi.jpg)
- **Config:** `Config::precise_ground()` with `CentroidMethod::MoffatFit { beta: 2.5 }`

The 2x slowdown vs v4 is expected: v4 used WeightedMoments (no fitting),
v5 uses Moffat L-M fitting per star. This is not a regression — it's a
different (more accurate) centroid method.

## Perf Configuration
- Sample rate: 3000 Hz, DWARF call graphs
- 620K samples collected across 10 benchmark iterations
- Event: `cpu_core/cycles/P`

## Top Functions by CPU Time (self %)

| Function | Self % | Children % | Notes |
|----------|--------|------------|-------|
| `lm_optimizer::optimize` | **21.71%** | **55.20%** | L-M optimizer (calls powf via model) |
| `libm::pow` (all offsets) | **~36%** | — | Moffat `u.powf(-beta)` calls |
| `background::interpolate_segment_avx2` | 6.09% | 6.33% | Background interpolation |
| `deblend::bfs_region` | 4.79% | 4.83% | BFS neighbor traversal |
| `deblend::deblend_multi_threshold` | 4.20% | 4.87% | Multi-threshold deblending |
| `threshold_mask::process_words_sse` | 3.92% | 3.96% | Threshold masking |
| `quicksort::partition` | 1.51% | — | Sorting operations |
| `centroid::refine_centroid` | 1.21% | 2.16% | Weighted moments (2 iters as seed) |
| `threshold_mask::process_words_filtered_sse` | 1.12% | 1.14% | Filtered threshold |
| `convolution::convolve_row_avx2` | 0.94% | 0.98% | Row convolution |
| `centroid::compute_metrics` | 0.92% | 0.93% | Star quality metrics |
| `centroid::linear_solver::solve` | 0.69% | 0.74% | Gaussian elimination (5x5/6x6) |
| `deblend::PixelGrid::reset_with_pixels` | 0.64% | — | Grid initialization |
| `centroid::measure_star` | 0.62% | — | Measurement entry point |

## Grouped by Pipeline Stage

| Stage | % CPU | Notes |
|-------|-------|-------|
| **Centroid fitting (optimize + libm)** | **~63%** | Dominated by `powf()` in Moffat model |
| Deblending | ~10% | bfs_region + multi_threshold + PixelGrid |
| Background | ~6% | interpolate_segment_avx2 |
| Threshold mask | ~5% | SSE vectorized |
| Sorting | ~2.3% | Quicksort partition |
| Rayon overhead | ~1.6% | Join/bridge overhead |
| Convolution | ~1.4% | AVX2 row convolution |
| Centroid (non-fitting) | ~2.8% | refine_centroid + compute_metrics |

## Root Cause Analysis: `powf()` Dominance

The Moffat profile model evaluates `u.powf(-beta)` which maps to `libm::pow`.
This single operation accounts for **~36% of total CPU time**.

### Why so expensive?

The generic L-M optimizer calls the model 3× per pixel per iteration:

1. **`fill_jacobian_residuals`** — calls `evaluate()` (1 `powf`) + `jacobian_row()` (1 `powf`)
2. **`compute_chi2(new_params)`** — calls `evaluate()` (1 `powf`) again for the trial step

With stamp radius = 6 (13×13 = 169 pixels) and up to 50 L-M iterations:
- **Per iteration:** 169 × 3 = 507 `powf` calls
- **Per star (5-15 iters typical):** 2,535 - 7,605 `powf` calls
- **`libm::pow` (f64):** ~20-30ns per call on modern x86

### Redundant computation

`fill_jacobian_residuals` already computes all residuals (which contain the
model evaluation). But `compute_chi2` re-evaluates the model from scratch for
the trial step. On a **successful** step, the chi² could be computed from the
already-known residuals. This would eliminate **1/3 of all `powf` calls**.

Additionally, `jacobian_row` and `evaluate` share most intermediate values
(`u`, `u_neg_beta`) but compute them independently.

## Optimization Opportunities

### 1. Eliminate redundant chi² evaluation (HIGH IMPACT, ~12% savings)

On a successful L-M step (accepted), `fill_jacobian_residuals` already computed
residuals. Sum their squares instead of calling `compute_chi2` again:

```
// After fill_jacobian_residuals, chi² is just sum of residuals²
let chi2_from_residuals: f64 = residuals.iter().map(|r| r * r).sum();
```

`compute_chi2` is still needed for the trial step (`new_params`), but we can
add a `LMModel::evaluate_chi2` or simply compute chi² inline from residuals
when available. This eliminates 1 of 3 `powf` per pixel on accepted iterations.

**Expected impact:** ~12% total runtime reduction (1/3 of the 36% libm cost).

### 2. Fused evaluate+jacobian (HIGH IMPACT, ~12% savings)

Add an `evaluate_and_jacobian` method to `LMModel` that computes both the
model value and Jacobian row in a single pass, sharing intermediate values:

```rust
fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; N]) -> (f64, [f64; N]) {
    // Compute u, u_neg_beta once, return both model value and jacobian
}
```

For Moffat, this means `u.powf(-beta)` is computed once instead of twice per
pixel in `fill_jacobian_residuals`.

**Expected impact:** ~12% total runtime reduction (eliminates 1 of 3 `powf`).

### 3. Fast `powf` for fixed beta (MEDIUM-HIGH IMPACT)

When beta is fixed (e.g., 2.5), `u.powf(-2.5)` can be decomposed:
```
u^(-2.5) = 1.0 / (u * u * u.sqrt())   // 1 sqrt + 3 muls vs 1 powf
```

`sqrt` is ~4ns vs `pow` at ~25ns. For beta = 3.0: `u^(-3) = 1/(u*u*u)`.
For general half-integer beta: decompose into integer power × sqrt.

This applies to `MoffatFixedBeta` which handles the common case.

**Expected impact:** 5-15% total runtime depending on beta decomposition speedup.

### 4. Reduce L-M iterations via better initial guess (MEDIUM IMPACT)

Currently the weighted moments seed runs only 2 iterations before L-M.
If moments converge well, L-M may need fewer iterations. Alternatively,
a linear least-squares initial estimate for amplitude/background could
reduce L-M iterations by 30-50%.

### 5. Combined opportunity: 1+2+3 together

Implementing optimizations 1, 2, and 3 together would reduce `powf` calls
from 3 per pixel per iteration to effectively ~0.3 equivalent cost:
- Fused evaluate+jacobian: 1 `powf` (was 2)
- Chi² from residuals: 0 extra `powf` on accepted steps (was 1)
- Fast `powf` decomposition: remaining call is ~4× cheaper

**Expected combined impact:** ~30-40% total runtime reduction (~525ms → ~450-525ms).

### 6. Background interpolation (LOW IMPACT)
- 6% of time, already AVX2 vectorized
- Diminishing returns

### 7. Deblending (LOW IMPACT)
- 10% of time, graph algorithms inherently sequential per component
- Already optimized with early termination and generation counters

## Optimizations Applied (v5a)

All three high-impact optimizations were implemented:

1. **Fused evaluate+jacobian** — `LMModel::evaluate_and_jacobian()` computes
   model value and Jacobian row in a single pass, sharing intermediate values
   (`u`, `u_neg_beta`, `exp_val`). Reduces `powf`/`exp` calls in
   `fill_jacobian_residuals` from 2 to 1 per pixel.

2. **Chi² from residuals** — `fill_jacobian_residuals` now returns chi² computed
   from the already-calculated residuals. On first iteration this replaces the
   separate `compute_chi2` call. `compute_chi2` is still used for trial steps.

3. **Fast `powf` for fixed beta** — `PowStrategy` enum pre-selects the optimal
   computation method at model construction:
   - Half-integer beta (2.5, 3.5, 4.5): `1 / (u^n * sqrt(u))` — sqrt + int_pow
   - Integer beta (2, 3, 4): `1 / u^n` — int_pow only
   - General: fallback to `u.powf(-beta)`

   For beta=2.5: `u^(-2.5) = 1 / (u² * √u)` — ~4ns vs ~25ns for `libm::pow`.

### Result

| Metric | Before | After | Change |
|--------|--------|-------|--------|
| Median | 726ms | 523ms | **-28.0%** |
| Min | 712ms | 512ms | -28.1% |
| Max | 743ms | 551ms | -25.8% |

## Comparison with Previous Profiles

| Version | Centroid Method | Median Time | Top Bottleneck |
|---------|----------------|-------------|----------------|
| v2 | WeightedMoments | 991ms | visit_neighbors (31%) |
| v3 | WeightedMoments | 408ms | visit_neighbors (14%) |
| v4 | WeightedMoments + SIMD | 393ms | visit_neighbors (14%) |
| v5 | MoffatFit f64 (pre-opt) | 726ms | lm_optimizer (55%) |
| v5a | MoffatFit f64 (optimized) | 523ms | TBD (re-profile) |
| **v5b** | **MoffatFit f64 + AVX2 SIMD** | **~420ms** (est.) | **TBD (re-profile)** |

v5/v5a use a fundamentally more expensive centroid method (L-M Moffat fitting
with f64 precision) than v4 (WeightedMoments). The fitting provides ~0.01 pixel
accuracy vs ~0.05 pixel for WeightedMoments.

## Optimizations Applied (v5b) — AVX2 SIMD Vectorization

4. **AVX2+FMA batch operations** — `MoffatFixedBeta` overrides `batch_fill_jacobian_residuals`
   and `batch_compute_chi2` with AVX2+FMA SIMD implementations that process 4 f64 pixels
   per iteration using `__m256d` registers. Key operations vectorized:
   - `_mm256_sqrt_pd` for `sqrt(u)` in HalfInt strategy
   - `_mm256_fmadd_pd` for fused multiply-add (r², u, model_val, chi² accumulation)
   - `simd_int_pow` for integer powers via repeated squaring in SIMD lanes
   - `simd_fast_pow_neg` dispatches per `PowStrategy` (HalfInt/Int/General)
   - Scalar tail handles remainder pixels (n % 4)
   - Runtime dispatch via `cpu_features::has_avx2_fma()` with scalar fallback

### Microbenchmark Results (SIMD)

| Benchmark | Before (v5a) | After (v5b) | Change |
|-----------|-------------|-------------|--------|
| fixed_beta_small (17×17) | 20.638µs | 12.484µs | **-39.5%** |
| fixed_beta_medium (25×25) | 43.379µs | 26.259µs | **-39.5%** |
| moffat_fit_single (21×21) | 20.652µs | 12.541µs | **-39.3%** |
| measure_star_moffat_fit | 11.429µs | 8.587µs | **-24.9%** |
| batch_6k_10000/moffat_fit | 59.278ms | 47.469ms | **-19.9%** |
| variable_beta (no SIMD) | 68.423µs | 67.933µs | -0.7% (unchanged) |

Variable beta is unaffected as expected (SIMD only targets `MoffatFixedBeta`).
Batch improvement is lower (~20%) because non-fitting overhead (stamp extraction,
moments seed, validation) dilutes the fitting speedup.

## Remaining Optimization Opportunities

- Re-profile to identify new bottleneck distribution after optimizations
- Explore reducing L-M iterations via better initial parameter estimates
- SIMD vectorize `compute_hessian_gradient` (J^T J accumulation)
- Consider SIMD for `MoffatVariableBeta` (would need SIMD `ln`/`exp` approximation)
