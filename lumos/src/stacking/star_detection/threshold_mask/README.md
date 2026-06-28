# Threshold Mask Module

SIMD-optimized binary mask creation for sigma thresholding in astronomical image processing.

## Overview

This module creates binary masks marking pixels above a sigma threshold relative to background and noise estimates. It is used by:

- **Background estimation**: Masking bright objects during iterative background refinement
- **Star detection**: Finding candidate pixels for connected-component labeling

## Architecture

```
threshold_mask/
├── mod.rs       # Public API, dispatch, and scalar kernel
├── avx2.rs      # x86_64 AVX2 SIMD implementation (preferred on x86_64)
├── sse.rs       # x86_64 SSE4.1 SIMD implementation
├── neon.rs      # ARM64 NEON SIMD implementation
├── tests.rs     # Unit tests
├── bench.rs     # Performance benchmarks
└── README.md    # This file
```

## Thresholding Variants

### Standard Threshold
```rust
// pixel > background + sigma * noise
create_threshold_mask(pixels, bg, noise, sigma, mask);
```

### Filtered (Background-Subtracted) Threshold
```rust
// filtered_pixel > sigma * noise
create_threshold_mask_filtered(filtered, noise, sigma, mask);
```

Both variants share one kernel via a `WITH_BG` const generic, so the background-load branch is
compiled out for the filtered path (which passes an empty `bg` slice).

## Implementation Details

### Bit-Packed Storage

Uses `BitBuffer2` for memory efficiency - 1 bit per pixel instead of 1 byte (8x memory reduction). This improves cache utilization for large images.

### SIMD Strategy

The implementation processes 64 pixels per word. SSE4.1/NEON use 16 groups of 4 pixels each (128-bit register width); AVX2 uses 8 groups of 8 pixels (256-bit), packing each group's 8-bit `_mm256_movemask_ps` result directly — half the iterations and no per-lane extract.

```
Word (64 bits) = 16 groups × 4 pixels/group   (SSE4.1 / NEON)
               = 8 groups × 8 pixels/group     (AVX2)
                 ↓
         [group0][group1]...
```

The trailing partial word (when a row's width isn't a multiple of 64) is handled by the shared scalar kernel (`process_words_scalar`), keeping the boundary semantics in one place across all backends.

**Per-group processing:**
1. Load 4 pixels, background, noise values
2. Compute threshold: `bg + sigma * max(noise, 1e-6)`
3. Compare: `pixels > threshold`
4. Extract comparison result as 4-bit mask
5. OR into word at appropriate bit position

### Platform-Specific Optimizations

| Platform | Implementation | Key Instructions |
|----------|----------------|------------------|
| x86_64   | AVX2 (preferred) | `_mm256_cmp_ps`, `_mm256_movemask_ps` |
| x86_64   | SSE4.1 (fallback) | `_mm_cmpgt_ps`, `_mm_movemask_ps` |
| ARM64    | NEON           | `vcgtq_f32`, `vgetq_lane_u32` |
| Other    | Scalar         | Bit-by-bit loop |

Backend selection on x86_64 is runtime feature detection (AVX2 → SSE4.1 → scalar).

**movemask advantage**: `_mm256_movemask_ps` / `_mm_movemask_ps` directly extract comparison results as an 8-bit / 4-bit integer, enabling efficient bit packing.

**NEON approach**: Extracts individual lane bits via `vgetq_lane_u32` since NEON lacks a direct movemask equivalent.

### Parallelization

Row-based parallel processing via Rayon. Each thread processes disjoint rows, writing to non-overlapping word ranges.

### Noise Floor Handling

Noise values are clamped to minimum `MIN_NOISE` (`1e-6`) to prevent division-by-zero-like artifacts when the noise estimate is zero or negative. The constant lives in `mod.rs` and is shared by every backend so they stay bit-exact.

## Performance

Benchmark results on 4K×4K image (16.7M pixels):

| Variant | SIMD | Scalar | Speedup |
|---------|------|--------|---------|
| Standard | 6.0ms | 19.3ms | 3.2× |
| Filtered | 4.3ms | 15.8ms | 3.7× |

Filtered is fastest (no background array access). Figures predate the AVX2 backend (SSE4.1-era).

## Best Practices Applied

Based on SIMD optimization literature:

1. **Hand-written intrinsics over auto-vectorization**: Compilers achieve only 45-71% vectorization success on complex patterns. Hand-tuned intrinsics ensure consistent performance.

2. **Aligned word boundaries**: Processing 64 pixels per word aligns with cache lines and simplifies remainder handling.

3. **Separate scalar fallback path**: Clean separation between SIMD fast path (full 64-pixel words) and scalar remainder handling.

4. **Memory access patterns**: Sequential reads through pixel/bg/noise arrays maximize cache efficiency.

5. **Branch-free SIMD core**: The comparison and bit-packing loop contains no branches in the SIMD path.

## References

- [Simd Library](https://github.com/ermig1979/Simd) - C++ SIMD image processing patterns
- [Rust Portable SIMD](https://doc.rust-lang.org/std/simd/index.html) - Portable SIMD abstractions
- [NEON vs SSE Comparison](https://blog.yiningkarlli.com/2021/09/neon-vs-sse.html) - Platform differences
- [SExtractor](https://www.astromatic.net/software/sextractor/) - Astronomical source detection with sigma thresholding
- [Astronomy & Astrophysics: Source Extraction](https://www.aanda.org/articles/aa/full_html/2021/01/aa36561-19/aa36561-19.html) - Background estimation methods
