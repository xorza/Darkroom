# Sum Module

Vector sum and accumulation operations with SIMD acceleration.

## Functions

| Function | Description | SIMD |
|----------|-------------|------|
| `sum_f32` | Sum all elements | AVX2, SSE4.1, NEON |
| `sum_squared_diff` | Sum of squared differences from mean | AVX2, SSE4.1, NEON |
| `mean_f32` | Arithmetic mean (uses `sum_f32`) | via sum |
| `accumulate` | Element-wise addition `dst[i] += src[i]` | scalar only |
| `scale` | Element-wise scaling `data[i] *= s` | scalar only |

## SIMD Implementation

### Why SIMD for sum operations?

Sum operations are **compute-bound** with high arithmetic intensity:
- Simple memory access pattern (sequential read)
- No write-back (read-only except final result)
- Horizontal reduction is the only overhead

### Benchmark Results (10,000 elements)

| Function | Scalar | SIMD (AVX2) | Speedup |
|----------|--------|-------------|---------|
| sum_f32 | 14.3 µs | 831 ns | **17x** |
| sum_squared_diff | 14.5 µs | 831 ns | **17x** |

The 17x speedup exceeds the theoretical 8x (AVX2 processes 8 floats per instruction) due to:
- Reduced loop overhead
- Better instruction-level parallelism
- Improved cache prefetching with larger strides

### Dispatch Hierarchy (x86_64)

```
values.len() >= 8 && has_avx2()  → avx2::sum_f32
values.len() >= 4 && has_sse4_1() → sse::sum_f32
otherwise                         → scalar::sum_f32
```

### Why no SIMD for accumulate/scale?

These are **memory-bound** operations (read + write to arrays). The compiler auto-vectorizes simple loops effectively, and explicit SIMD adds complexity without measurable benefit.

## Files

- `mod.rs` - Public API with SIMD dispatch
- `scalar.rs` - Scalar implementations
- `sse.rs` - SSE4.1 implementations (x86_64)
- `avx2.rs` - AVX2 implementations (x86_64)
- `neon.rs` - NEON implementations (aarch64)
- `tests.rs` - Unit tests including SIMD boundary tests
- `bench.rs` - Benchmarks comparing scalar vs SIMD
