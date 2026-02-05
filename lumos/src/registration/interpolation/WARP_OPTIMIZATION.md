# Warp Optimization Analysis

## Current Benchmarks (After LUT Optimization)

| Benchmark | Before | After | Improvement |
|-----------|--------|-------|-------------|
| `bench_warp_lanczos3_1k` | 8.2ms | 6.3ms | **-23%** |
| `bench_warp_lanczos3_2k` | 31.8ms | 25.0ms | **-21%** |
| `bench_warp_lanczos3_4k` | 123ms | 104ms | **-15%** |
| `bench_warp_bilinear_2k` | 2.8ms | 2.9ms | (baseline) |
| `bench_interpolate_lanczos3_single` | 70µs | 49µs | **-30%** |
| `bench_lut_lookup` | 4.5µs | 2.8µs | **-38%** |

**Lanczos3 vs Bilinear ratio:** ~9x slower (25ms vs 2.9ms for 2k)

## Optimizations Applied

### 1. LUT Resolution Increase + Direct Lookup (2025-02-05)
- Increased LUT resolution from 1024 to 4096 entries
- Removed linear interpolation, using direct indexing with rounding
- Result: 20-30% faster across all Lanczos benchmarks

## Baseline Benchmarks (Before Optimizations)

| Benchmark | Time | Notes |
|-----------|------|-------|
| `bench_warp_lanczos3_1k` | 8.2ms | 1024x1024, single channel |
| `bench_warp_lanczos3_2k` | 31.8ms | 2048x2048, single channel |
| `bench_warp_lanczos3_4k` | 123ms | 4096x4096, single channel |
| `bench_warp_bilinear_2k` | 2.8ms | 2048x2048, SIMD-accelerated |
| `bench_interpolate_lanczos3_single` | 70µs | 1000 pixel interpolations |
| `bench_lut_lookup` | 4.5µs | 1000 LUT lookups |

## Current Performance Profile

From perf profiling of `bench_warp` (Lanczos3, ~6000x4000 RGB image):

| Function | Self % | Description |
|----------|--------|-------------|
| `interpolate_lanczos` | 85.09% | Main hotspot - Lanczos kernel evaluation |

**Current time:** ~600ms per warp (3 channels, ~6000x4000)

## Bottleneck Analysis

The `interpolate_lanczos_impl` function dominates execution time due to:

1. **Per-pixel LUT lookups** - 12 LUT lookups per pixel (6 for X weights, 6 for Y weights for Lanczos3)
2. **Linear interpolation in LUT** - Each lookup involves float conversion, multiply, add
3. **36 multiply-accumulates** - 6x6 kernel window for each output pixel
4. **Non-SIMD scalar code** - All operations are scalar despite being highly parallelizable

## Remaining Optimization Strategies

### ~~Separable Filter~~ (NOT APPLICABLE)

The separable filter optimization only works for **pure scaling/resampling** where source coordinates form a regular grid. The current `warp_image` applies arbitrary geometric transforms (rotation, skew, translation), meaning each output pixel maps to a unique arbitrary location via `inverse.apply()`. **This cannot be separated into horizontal/vertical passes.**

### ~~SIMD Vectorization~~ (ALREADY TRIED - MINIMAL BENEFIT)

From `simd/mod.rs`:
> "Lanczos3: Scalar only (SIMD provides <5% improvement due to memory-bound LUT lookups)"

SIMD for Lanczos was implemented and removed. The bottleneck is memory-bound LUT lookups (12 per pixel for Lanczos3), not compute. SIMD gather instructions have high latency and don't help.

### 1. Polynomial Approximation (Medium Impact - Estimated 1.5-2x)

Replace LUT with fast polynomial approximation of the Lanczos kernel:
- **No memory access** - purely compute-bound, can benefit from SIMD
- Trade accuracy for speed (acceptable for most use cases)
- Use Chebyshev or minimax polynomial fit

### 2. Reduce Border Checking (Low-Medium Impact)

**Current:** Bounds checking for every sample (36 checks per pixel for Lanczos3)
**Proposed:**
- Pre-compute interior region where no bounds checks needed
- Use fast path for interior, slow path only for edges
- Could save 10-20% by eliminating branches in hot loop

### 3. Batch Transform Computation (Low Impact)

**Current:** `inverse.apply()` called per pixel
**Proposed:** 
- For affine transforms, compute row start + delta per pixel
- Reduce matrix multiply overhead

### 4. Consider Bilinear for Non-Critical Cases

Bilinear is 9x faster (25ms vs 2.9ms for 2k). For preview/interactive use, consider:
- Bilinear for interactive preview
- Lanczos only for final output

## Summary

| Optimization | Status | Impact |
|-------------|--------|--------|
| ✅ LUT optimization | Done | -20-30% |
| ❌ Separable filter | Not applicable (arbitrary transform) | N/A |
| ❌ SIMD | Already tried, <5% improvement | N/A |
| ⏳ Polynomial approx | Next candidate | Est. 1.5-2x |
| ⏳ Border check fast path | Possible | Est. 10-20% |

**Current performance is likely near-optimal for Lanczos with arbitrary transforms.**

## Code References

- Bilinear SIMD implementation exists in `simd.rs` - can serve as template
- Intel IPP uses `_mm256_insertf128_ps` for packing, `_mm256_hadd_ps` for reduction
- Consider using `std::arch` directly or `packed_simd2` / `wide` crates

## Additional Resources

- [Intel IPP Lanczos AVX](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html)
- [AVIR Library (C++ SIMD Lanczos)](https://github.com/avaneev/avir)
- [Lanczos Resampling Explained](https://mazzo.li/posts/lanczos.html)
- [Efficient Lanczos on ARM NEON](https://www.researchgate.net/publication/322881948_Efficient_Projective_Transformation_and_Lanczos_Interpolation_on_ARM_Platform_using_SIMD_Instructions)
