# Deviation Module

Computes absolute deviations from a central value (typically median).

## Functions

| Function | Description | SIMD |
|----------|-------------|------|
| `abs_deviation_inplace` | Replace values with `\|value - median\|` | None (scalar only) |

## Why No SIMD?

SIMD implementations (SSE4.1, AVX2, NEON) were **removed** after benchmarking showed they provided no benefit or were slower than scalar code.

### Benchmark Results (10,000 elements)

| Implementation | Time |
|----------------|------|
| Scalar | 1.2 µs |
| SIMD (AVX2) | 2.4 µs |

**SIMD was 2x slower** than scalar.

### Root Cause: Memory-Bound Operation

`abs_deviation_inplace` is **memory-bandwidth limited**, not compute-limited:

1. **Read + Write pattern**: Each element must be read and written back
2. **Trivial compute**: Just `subtract + abs` (2 operations per element)
3. **Memory dominates**: Time is spent waiting for memory, not computing

The operation's arithmetic intensity is too low to benefit from SIMD:
```
Arithmetic Intensity = FLOPS / Bytes
                     = 2 ops / 8 bytes (read + write)
                     = 0.25 ops/byte
```

For comparison, `sum_f32` has much higher intensity:
```
Arithmetic Intensity = 1 op / 4 bytes (read only)
                     = 0.25 ops/byte initially
                     but accumulates into register → effectively higher
```

### Compiler Auto-Vectorization

Modern compilers (LLVM/rustc) auto-vectorize simple loops like:
```rust
for v in values.iter_mut() {
    *v = (*v - median).abs();
}
```

The scalar code already benefits from compiler optimizations without explicit SIMD intrinsics.

### Removed Files

The following SIMD implementations were deleted:
- `sse.rs` - SSE4.1 implementation
- `avx2.rs` - AVX2 implementation  
- `neon.rs` - NEON implementation (aarch64)

## Files

- `mod.rs` - Public API (delegates to scalar)
- `scalar.rs` - Scalar implementation
- `tests.rs` - Unit tests
- `bench.rs` - Performance benchmark
