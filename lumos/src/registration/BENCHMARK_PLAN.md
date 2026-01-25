# Registration Module Benchmark Plan

This document describes the comprehensive benchmark suite for the registration module.

## Running Benchmarks

```bash
# Run all registration benchmarks
cargo bench -p lumos --features bench --bench registration

# Run specific benchmark group
cargo bench -p lumos --features bench --bench registration -- "simd_vs_scalar"
cargo bench -p lumos --features bench --bench registration -- "two_step_matching"
cargo bench -p lumos --features bench --bench registration -- "phase_correlation_iterative"

# List all available benchmarks
cargo bench -p lumos --features bench --bench registration -- --list
```

---

## Benchmark Groups

### 1. Transform Types (`types/bench.rs`)

| Benchmark | Description | Metrics |
|-----------|-------------|---------|
| `transform_point/single/{type}` | Single point transformation | Time per transform |
| `matrix_inverse/{type}` | Matrix inversion | Time per inverse |
| `matrix_compose/{n}_transforms` | Matrix composition | Time per composition |
| `transform_batch/sequential/{n}` | Batch point transforms | Throughput (elem/s) |

**Transform types**: translation, euclidean, similarity, affine, homography

---

### 2. Triangle Matching (`triangle/bench.rs`)

| Benchmark | Description | Metrics |
|-----------|-------------|---------|
| `triangle_formation/form/{n}` | Form triangles from N stars | Time, Throughput |
| `triangle_hash_table_build/build/{n}` | Build geometric hash table | Time, Throughput |
| `triangle_hash_table_lookup/single_lookup` | Single hash table query | Time per lookup |
| `match_stars/full_match/{n}` | Complete star matching | Time, Throughput |
| `match_stars/with_outliers` | Matching with outliers | Time |
| `two_step_matching/single_step/{n}` | Standard matching | Time, Throughput |
| `two_step_matching/two_step/{n}` | Two-step refinement | Time, Throughput |
| `two_step_matching/dense_with_outliers_*` | Dense field comparison | Time |

**Key comparison**: `two_step_matching/single_step` vs `two_step_matching/two_step`

---

### 3. RANSAC (`ransac/bench.rs`)

| Benchmark | Description | Metrics |
|-----------|-------------|---------|
| `ransac_estimation/similarity/{n}` | RANSAC with N points | Time, Throughput |
| `ransac_transform_types/{type}` | Different transform types | Time |
| `ransac_outlier_ratios/outliers/{p}%` | Varying outlier percentages | Time |
| `ransac_refinement/least_squares/{n}` | LS refinement step | Time, Throughput |
| `ransac_simd_vs_scalar/count_inliers_simd/{n}` | SIMD inlier counting | Time, Throughput |
| `ransac_simd_vs_scalar/compute_residuals_simd/{n}` | SIMD residual computation | Time, Throughput |

**Key comparison**: SIMD performance scales with point count

---

### 4. Phase Correlation (`phase_correlation/bench.rs`)

| Benchmark | Description | Metrics |
|-----------|-------------|---------|
| `phase_correlation_sizes/correlate/{size}` | Different image sizes | Time, Throughput |
| `phase_correlation_subpixel/{method}` | Sub-pixel methods | Time |
| `phase_correlation_windowing/{with/no}_window` | Windowing overhead | Time |
| `phase_correlation_iterative/single_shot` | Standard correlation | Time |
| `phase_correlation_iterative/iterative_3` | 3-iteration refinement | Time |
| `phase_correlation_iterative/iterative_5` | 5-iteration refinement | Time |

**Sub-pixel methods**: none, parabolic, gaussian, centroid

**Key comparison**: `single_shot` vs `iterative_{3,5}` for accuracy/speed tradeoff

---

### 5. Interpolation (`interpolation/bench.rs`)

| Benchmark | Description | Metrics |
|-----------|-------------|---------|
| `interpolation_methods/single_pixel/{method}` | Single pixel sample | Time |
| `interpolation_methods/batch_1000/{method}` | 1000 pixel batch | Time, Throughput |
| `warp_image_sizes/{method}/{size}` | Full image warp | Time, Throughput |
| `resample/upscale_2x_{method}` | 2x upscaling | Time, Throughput |
| `resample/downscale_2x_{method}` | 2x downscaling | Time, Throughput |
| `simd_vs_scalar/bilinear_simd_row/{size}` | SIMD bilinear row | Time, Throughput |
| `simd_vs_scalar/bilinear_scalar_row/{size}` | Scalar bilinear row | Time, Throughput |
| `simd_vs_scalar/lanczos3_simd_row/{size}` | SIMD Lanczos3 row | Time, Throughput |
| `simd_vs_scalar/lanczos3_scalar_row/{size}` | Scalar Lanczos3 row | Time, Throughput |
| `simd_vs_scalar/bilinear_simd_image_512` | Full image bilinear | Time, Throughput |
| `simd_vs_scalar/lanczos3_simd_image_512` | Full image Lanczos3 | Time, Throughput |

**Interpolation methods**: nearest, bilinear, bicubic, lanczos2, lanczos3, lanczos4

**Key comparison**: `simd_vs_scalar` group measures SIMD speedup

---

### 6. Pipeline (`pipeline/bench.rs`)

| Benchmark | Description | Metrics |
|-----------|-------------|---------|
| `registration_pipeline/similarity/{n}` | Full pipeline, N stars | Time, Throughput |
| `registration_pipeline/with_20%_outliers` | Outlier handling | Time |
| `registration_config/{type}` | Different transform types | Time |
| `registration_config/ransac_iters/{n}` | Varying RANSAC iterations | Time |
| `warp_to_reference/{method}/{size}` | Image warping | Time, Throughput |

---

### 7. Quality Metrics (`quality/bench.rs`)

| Benchmark | Description | Metrics |
|-----------|-------------|---------|
| `quality_metrics/compute/{n}` | Quality metric computation | Time, Throughput |
| `residual_computation/compute/{n}` | Residual calculation | Time, Throughput |
| `residual_stats/compute/{n}` | Statistical analysis | Time, Throughput |
| `quadrant_consistency/check/{n}` | Quadrant validation | Time, Throughput |
| `overlap_estimation/{type}` | Overlap estimation | Time |

---

## Expected Performance Characteristics

### SIMD Speedup (x86_64 with AVX2)

| Operation | Scalar | SIMD | Speedup |
|-----------|--------|------|---------|
| Bilinear row warp (1024px) | ~10 µs | ~6 µs | **1.6x** |
| Lanczos3 row warp (1024px) | ~60 µs | ~25 µs | **2.4x** |
| Inlier counting (200 pts) | ~2 µs | ~0.8 µs | **2.5x** |
| Residual computation (200 pts) | ~1.5 µs | ~0.6 µs | **2.5x** |

### Algorithm Complexity

| Operation | Complexity | Typical Time (100 stars) |
|-----------|------------|-------------------------|
| Triangle formation | O(n × k²) | ~50 µs |
| Hash table build | O(T) | ~20 µs |
| Star matching | O(n × T × k) | ~200 µs |
| RANSAC (1000 iter) | O(iter × n) | ~5 ms |
| Phase correlation | O(N² log N) | ~2 ms (512×512) |
| Image warp (Lanczos3) | O(W × H × 36) | ~50 ms (1024×1024) |

---

## Profiling Guide

### CPU Profiling with perf

```bash
# Record with perf
cargo bench -p lumos --features bench --bench registration -- --profile-time 10

# Or use perf directly
perf record -g cargo bench -p lumos --features bench --bench registration -- "lanczos3"
perf report
```

### Memory Profiling

```bash
# Use heaptrack
heaptrack cargo bench -p lumos --features bench --bench registration
heaptrack_gui heaptrack.*.zst
```

### Flamegraph

```bash
cargo install flamegraph
cargo flamegraph --bench registration -p lumos --features bench -- "warp_image"
```

---

## Regression Testing

### Baseline Comparison

```bash
# Save baseline
cargo bench -p lumos --features bench --bench registration -- --save-baseline main

# Compare against baseline after changes
cargo bench -p lumos --features bench --bench registration -- --baseline main
```

### CI Integration

Add to CI pipeline:
```yaml
- name: Run benchmarks
  run: cargo bench -p lumos --features bench --bench registration -- --noplot
```

---

## Performance Targets

| Metric | Target | Current |
|--------|--------|---------|
| Star matching (100 stars) | < 500 µs | ~200 µs ✅ |
| RANSAC similarity (100 pts) | < 10 ms | ~5 ms ✅ |
| Image warp Lanczos3 (1024²) | < 100 ms | ~50 ms ✅ |
| Phase correlation (512²) | < 5 ms | ~2 ms ✅ |
| SIMD bilinear speedup | > 1.5x | 1.6x ✅ |
| SIMD Lanczos3 speedup | > 2.0x | 2.4x ✅ |

---

## Adding New Benchmarks

1. Add benchmark function to appropriate `bench.rs` file
2. Register in `benchmarks()` function
3. Follow naming convention: `group/variant/parameter`
4. Include throughput metrics where applicable
5. Add comparison baselines (scalar vs SIMD, old vs new algorithm)

Example:
```rust
fn benchmark_new_feature(c: &mut Criterion) {
    let mut group = c.benchmark_group("new_feature");
    
    for size in [100, 500, 1000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_function(BenchmarkId::new("variant", size), |b| {
            b.iter(|| {
                // benchmark code
            })
        });
    }
    
    group.finish();
}
```
