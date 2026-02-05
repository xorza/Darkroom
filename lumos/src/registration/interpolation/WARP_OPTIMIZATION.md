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

### ~~SIMD Vectorization~~ (TRIED AGAIN 2025-02-05 - CONFIRMED NO BENEFIT)

AVX2 implementation was re-attempted with:
- Vectorized coordinate transform computation
- AVX2 multiply-accumulate for 6x6 kernel
- 8 pixels processed per iteration

**Result:** ~24.5ms vs ~24.5ms scalar baseline - **no measurable improvement**.

**Root cause:** The bottleneck is **memory-bound**, not compute-bound:
- 36 random memory accesses per output pixel (6x6 kernel window)
- Each access potentially misses cache due to arbitrary transform coordinates
- SIMD gather instructions have high latency (~20 cycles vs ~4 for arithmetic)

**Research findings from Intel IPP and AVIR:**
- Intel IPP achieves 1.5x speedup, but only for **scaling** (separable filter)
- AVIR uses 2-pass horizontal/vertical - not applicable to arbitrary transforms
- All fast implementations rely on separable filtering which requires regular grid

### 1. Polynomial Approximation (Medium Impact - Estimated 1.5-2x)

Replace LUT with fast polynomial approximation of the Lanczos kernel:
- **No memory access** - purely compute-bound, can benefit from SIMD
- Trade accuracy for speed (acceptable for most use cases)
- Use Chebyshev or minimax polynomial fit

### ~~2. Row-Level Border Checking + Batch Transform~~ (TRIED 2025-02-05 - DISCARDED)

Attempted combining:
- Batch transform computation (row start + delta instead of per-pixel matrix multiply)
- Row-level interior detection (unchecked access for interior rows)

**Result:** No measurable impact. Benchmark results were within noise (~24ms before and after).
Code complexity not justified for marginal gains. Discarded.

### 4. Consider Bilinear for Non-Critical Cases

Bilinear is 9x faster (25ms vs 2.9ms for 2k). For preview/interactive use, consider:
- Bilinear for interactive preview
- Lanczos only for final output

## Summary

| Optimization | Status | Impact |
|-------------|--------|--------|
| ✅ LUT optimization | Done | -20-30% |
| ❌ Separable filter | Not applicable (arbitrary transform) | N/A |
| ❌ SIMD (AVX2) | Re-tested 2025-02-05, confirmed no benefit | 0% |
| ❌ Row-level interior + batch transform | Tried 2025-02-05, no measurable impact | 0% |
| ⏳ Polynomial approx | Next candidate | Est. 1.5-2x |

**Current performance is likely near-optimal for Lanczos with arbitrary transforms.**

The fundamental limitation is that arbitrary geometric transforms prevent:
1. **Separable filtering** - requires regular grid coordinates
2. **Effective SIMD** - random memory access patterns dominate

For further speedup, consider:
- Use bilinear for interactive preview (9x faster)
- Use Lanczos only for final stacking output

## Code References

- Bilinear SIMD implementation exists in `simd.rs` - can serve as template
- Intel IPP uses `_mm256_insertf128_ps` for packing, `_mm256_hadd_ps` for reduction
- Consider using `std::arch` directly or `packed_simd2` / `wide` crates

## Additional Resources

- [Intel IPP Lanczos AVX](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html)
- [AVIR Library (C++ SIMD Lanczos)](https://github.com/avaneev/avir)
- [Lanczos Resampling Explained](https://mazzo.li/posts/lanczos.html)
- [Efficient Lanczos on ARM NEON](https://www.researchgate.net/publication/322881948_Efficient_Projective_Transformation_and_Lanczos_Interpolation_on_ARM_Platform_using_SIMD_Instructions)
