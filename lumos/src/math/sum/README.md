# Sum Module

Vector sum and accumulation operations with SIMD acceleration.

## Functions

| Function | Description | SIMD |
|----------|-------------|------|
| `sum_f32` | Sum all elements | AVX2, NEON |
| `mean_f32` | Arithmetic mean (uses `sum_f32`) | via sum |
| `weighted_mean_f32` | Weighted mean | AVX2, SSE4.1, NEON |

## SIMD Implementation

### Dispatch Hierarchy (x86_64)

```
sum:
values.len() >= 256 && has_avx2() → avx2::sum_f32
otherwise                          → scalar::sum_f32

weighted mean:
values.len() >= 128 && has_avx2()   → avx2::weighted_mean_f32
values.len() >= 128 && has_sse4_1() → sse::weighted_mean_f32
otherwise                           → scalar::weighted_mean_f32
```

The scalar kernels use `f64` products and accumulation before rounding once to `f32`. The SIMD
kernels retain compensated `f32` accumulation and are selected only beyond their measured
crossover points.

On AArch64, NEON is used for arrays of 4+ elements.

## Files

- `mod.rs` - Public API with SIMD dispatch
- `scalar.rs` - Scalar implementations
- `sse.rs` - SSE4.1 weighted mean (x86_64)
- `avx2.rs` - AVX2 implementations (x86_64)
- `neon.rs` - NEON implementations (aarch64)
- `tests.rs` - Unit tests including SIMD boundary and weighted_mean tests
- `bench.rs` - Scalar/SIMD crossover and large-slice benchmarks
