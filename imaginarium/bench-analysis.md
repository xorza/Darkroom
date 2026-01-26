# Imaginarium Conversion Benchmark Analysis

## Summary (4096×4096 = 16.7M pixels)

| Conversion | Time | Throughput | Implementation |
|------------|------|------------|----------------|
| **Channel U8** |
| RGBA→RGB | 22.2ms | 757 Melem/s | SSSE3 PSHUFB |
| RGB→RGBA | 21.3ms | 789 Melem/s | SSSE3 PSHUFB |
| **Channel F32** |
| RGBA→RGB | 79.9ms | 210 Melem/s | Scalar+Rayon |
| RGB→RGBA | 78.5ms | 214 Melem/s | Scalar+Rayon |
| **Luminance U8** |
| RGBA→L | 17.7ms | 947 Melem/s | PMADDUBSW |
| RGB→L | 14.4ms | 1.16 Gelem/s | PMADDUBSW |
| L→RGBA | 16.7ms | 1.00 Gelem/s | Broadcast |
| **Luminance F32** |
| RGBA→L | 58.5ms | 286 Melem/s | SSE shuffle+FMA |
| L→RGBA | 56.8ms | 295 Melem/s | SSE broadcast |
| **Bit Depth** |
| U8→F32 | 57.4ms | 292 Melem/s | SSE2 unpack |
| F32→U8 | 59.3ms | 283 Melem/s | SSE2 pack |
| U8→U16 | 36.0ms | 466 Melem/s | Bit replication |
| U16→U8 | 36.1ms | 465 Melem/s | High byte |

## Throughput by Image Size

| Conversion | 256×256 | 1024×1024 | 4096×4096 |
|------------|---------|-----------|-----------|
| RGB→L U8 | 1.35 Gelem/s | 1.98 Gelem/s | 1.16 Gelem/s |
| L→RGBA U8 | 1.44 Gelem/s | 2.15 Gelem/s | 1.00 Gelem/s |
| RGBA→RGB U8 | 963 Melem/s | 1.82 Gelem/s | 757 Melem/s |
| U8→U16 | 779 Melem/s | 1.25 Gelem/s | 466 Melem/s |
| U8→F32 | 669 Melem/s | 460 Melem/s | 292 Melem/s |

**Peak at 1024×1024**: Data fits in L3 cache (~4MB), parallelization overhead amortized.

## SIMD Optimization Summary

| Phase | Conversion | Improvement | Technique |
|-------|-----------|-------------|-----------|
| 1 | RGBA↔RGB U8 | 1.5-3.2x | SSSE3 `_mm_shuffle_epi8` |
| 2 | Luminance U8 | 1.6-3.1x | `_mm_maddubs_epi16` |
| 3 | U8↔F32 | 1.35-1.6x | SSE2 unpack + convert |
| 4 | U8↔U16 | 1.35-1.8x | `(val << 8) | val` |
| 5 | F32 channels | 1.1-1.4x | Rayon parallel |
| 6 | F32 Luminance | 1.1-1.25x | SSE shuffle + FMA |

## Key Insights

### Memory Bandwidth Limits

For 4096×4096 RGBA_U8:
- Input: 67MB, Output: 50MB
- Measured: ~5.3 GB/s effective
- DDR4-3200 practical: ~15-20 GB/s

Large images are memory-bound; SIMD reduces instructions but can't exceed memory bandwidth.

### F32 vs U8 Performance

| Format | RGBA→RGB | Ratio |
|--------|----------|-------|
| U8 | 757 Melem/s | 1.0x |
| F32 | 210 Melem/s | 0.28x |

F32 is 3-4x slower due to 4x data size.

### Luminance is Fastest

RGB→L achieves 1.16 Gelem/s because output is 1/3 or 1/4 the input size, reducing write bandwidth.

## Implementation Details

### SSE/SSSE3 Techniques

| Technique | Intrinsic | Use |
|-----------|-----------|-----|
| Byte shuffle | `_mm_shuffle_epi8` | RGBA↔RGB reorder |
| Weighted sum | `_mm_maddubs_epi16` | Luminance |
| Zero-extend | `_mm_unpacklo_epi8` | U8→U16/U32 |
| Float convert | `_mm_cvtepi32_ps` | U32→F32 |
| Pack saturate | `_mm_packus_epi16` | U16→U8 |
| Blend | `_mm_blend_ps` | L→RGBA alpha |

### Luminance Weights (Rec. 709)

Fixed-point for U8: R=54, G=183, B=19 (sum=256, shift by 8)
Floating-point for F32: R=0.2126, G=0.7152, B=0.0722

## Recommendations

### Keep Images in U8/U16

Convert to F32 only when processing requires it. F32 is 3-4x slower.

### Use 1024×1024 Tiles for Large Images

Peak throughput at cache-friendly sizes. Consider tile-based processing.

### AVX2 for Modern CPUs

89%+ CPU support (Steam survey). Could provide ~2x throughput over SSE.
