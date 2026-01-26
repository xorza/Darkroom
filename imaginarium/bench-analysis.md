# Imaginarium Conversion Benchmark Analysis

## Summary Table (4096×4096 images - 16.7M pixels)

| Conversion | Time (ms) | Throughput | Notes |
|------------|-----------|------------|-------|
| **Channel Conversions (U8) - SIMD** |
| RGBA→RGB U8 | 22.2 | 757 Melem/s | SSSE3 PSHUFB shuffle |
| RGB→RGBA U8 | 21.3 | 789 Melem/s | SSSE3 PSHUFB shuffle |
| **Channel Conversions (F32) - Rayon** |
| RGBA→RGB F32 | 110.5 | 152 Melem/s | Scalar + parallel |
| RGB→RGBA F32 | 79.8 | 210 Melem/s | Scalar + parallel |
| **Bit Depth (U8↔F32) - SIMD** |
| RGBA U8→F32 | 57.4 | 292 Melem/s | SSE2 unpack + convert |
| RGBA F32→U8 | 59.3 | 283 Melem/s | SSE2 convert + pack |
| RGB U8→F32 | 42.1 | 399 Melem/s | Scalar + parallel |
| RGB F32→U8 | 43.8 | 383 Melem/s | Scalar + parallel |
| **Bit Depth (U8↔U16) - SIMD** |
| RGBA U8→U16 | 36.0 | 466 Melem/s | Bit replication |
| RGBA U16→U8 | 36.1 | 465 Melem/s | High byte extraction |
| **Luminance (U8) - SIMD** |
| RGBA→L U8 | 17.7 | 947 Melem/s | PMADDUBSW Rec.709 |
| RGB→L U8 | 14.4 | 1.16 Gelem/s | PMADDUBSW Rec.709 |
| L→RGBA U8 | 16.7 | 1.00 Gelem/s | Broadcast expansion |
| **Luminance (F32) - Scalar** |
| RGBA→L F32 | 60.0 | 280 Melem/s | Floating-point weights |
| L→RGBA F32 | 57.5 | 291 Melem/s | Broadcast expansion |

## Performance by Image Size

| Conversion | 256×256 | 1024×1024 | 4096×4096 |
|------------|---------|-----------|-----------|
| RGBA→RGB U8 | 963 Melem/s | 1.82 Gelem/s | 757 Melem/s |
| RGB→RGBA U8 | 1.08 Gelem/s | 1.85 Gelem/s | 789 Melem/s |
| RGBA→L U8 | 1.26 Gelem/s | 1.90 Gelem/s | 947 Melem/s |
| RGB→L U8 | 1.35 Gelem/s | 1.98 Gelem/s | 1.16 Gelem/s |
| L→RGBA U8 | 1.44 Gelem/s | 2.15 Gelem/s | 1.00 Gelem/s |
| U8→U16 | 779 Melem/s | 1.25 Gelem/s | 466 Melem/s |
| U8→F32 | 669 Melem/s | 460 Melem/s | 292 Melem/s |

**Observation**: 1024×1024 often shows peak throughput because:
- Data fits well in L3 cache (~4-8MB for 1024×1024 RGBA)
- Rayon parallelization overhead is amortized
- 4096×4096 (67MB+) exceeds cache, becomes memory-bound

## SIMD Optimization Results

| Phase | Conversion | Improvement | Technique |
|-------|-----------|-------------|-----------|
| 1 | RGBA↔RGB U8 | 1.5-3.2x | SSSE3 `_mm_shuffle_epi8` (16 pixels/iter) |
| 2 | Luminance U8 | 1.6-3.1x | `_mm_maddubs_epi16` weighted sum |
| 3 | U8↔F32 | 1.35-1.6x | SSE2 unpack + `_mm_cvtepi32_ps` |
| 4 | U8↔U16 | 1.35-1.8x | `(val << 8) | val` bit replication |
| 5 | F32 channels | 1.1-1.4x | Rayon parallelization (complex shuffle) |

### Techniques Used

- **RGBA↔RGB U8**: SSSE3 `PSHUFB` for byte permutation, processes 16 pixels per loop iteration
- **Luminance**: Fixed-point Rec.709 weights (R=54, G=183, B=19, sum=256) with `PMADDUBSW` + shift-by-8
- **U8→F32**: Zero-extend bytes to u32 via `_mm_unpacklo_epi8`, then `_mm_cvtepi32_ps`
- **U8→U16**: Proper 8→16 bit expansion: `val * 257 = (val << 8) | val` (0xFF → 0xFFFF)
- **F32 channels**: Scalar with Rayon - F32 shuffle is complex without SSE4.1 blend, and memory bandwidth is the bottleneck anyway

## Key Observations

### 1. Memory Bandwidth is the Bottleneck (Large Images)

For 4096×4096 RGBA_U8 (67MB input + 50MB output):
- Measured: ~5.3 GB/s effective bandwidth
- DDR4-3200 theoretical: ~25 GB/s
- DDR4-3200 practical: ~15-20 GB/s

SIMD reduces instruction count but can't exceed memory bandwidth on large images.

### 2. F32 Operations are 3-5x Slower than U8

| Format | RGBA→RGB | Ratio |
|--------|----------|-------|
| U8 | 757 Melem/s | 1.0x |
| F32 | 152 Melem/s | 0.2x |

This is expected: F32 has 4x the data size, and floating-point operations are more complex.

### 3. Luminance is Fastest (Output Reduction)

RGB→L U8 achieves 1.16 Gelem/s because output is 1/3 or 1/4 the size of input, reducing memory write bandwidth.

### 4. Cache-Friendly Sizes Show Best SIMD Gains

| Size | Cache Fit | SIMD Benefit |
|------|-----------|--------------|
| 256×256 (256KB) | L2 | Good (1.5-2x) |
| 1024×1024 (4MB) | L3 | Best (2-3x) |
| 4096×4096 (67MB) | RAM | Limited (1.2-1.5x) |

## Recommendations

### Completed ✅

1. **SIMD for high-priority conversions** (Phases 1-5)

### Future Optimizations

2. **AVX2/AVX-512 paths** for wider SIMD (256/512-bit)
   - Potential 2-4x additional speedup on supported CPUs

3. **F32 Luminance SIMD** - currently scalar
   - Use `_mm_dp_ps` (SSE4.1) or FMA for dot product

4. **Tile-based processing** for better cache utilization
   - Process 64×64 tiles to keep data in L2 cache

5. **Buffer reuse API** to avoid allocation overhead
   - `Image::new_black()` allocates on each conversion

6. **Keep images in U8/U16** when possible
   - Convert to F32 only for processing that requires it
