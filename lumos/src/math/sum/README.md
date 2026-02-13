# Sum Module

Vector sum and accumulation operations with SIMD acceleration.

## Functions

| Function | Description | SIMD |
|----------|-------------|------|
| `sum_f32` | Sum all elements | AVX2, SSE4.1, NEON |
| `mean_f32` | Arithmetic mean (uses `sum_f32`) | via sum |
| `weighted_mean_f32` | Weighted mean | scalar only |

## SIMD Implementation

### Dispatch Hierarchy (x86_64)

```
values.len() >= 8 && has_avx2()  → avx2::sum_f32
values.len() >= 4 && has_sse4_1() → sse::sum_f32
otherwise                         → scalar::sum_f32
```

On aarch64, NEON is used for arrays of 4+ elements.

## Files

- `mod.rs` - Public API with SIMD dispatch
- `scalar.rs` - Scalar implementations
- `sse.rs` - SSE4.1 implementations (x86_64)
- `avx2.rs` - AVX2 implementations (x86_64)
- `neon.rs` - NEON implementations (aarch64)
- `tests.rs` - Unit tests including SIMD boundary and weighted_mean tests
- `bench.rs` - Benchmarks comparing scalar vs SIMD
