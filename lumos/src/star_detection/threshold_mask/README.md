# Threshold Mask Module

SIMD-optimized binary mask creation for sigma thresholding in astronomical image processing.

## Overview

This module creates binary masks marking pixels above a sigma threshold relative to background and noise estimates. It is used by:

- **Background estimation**: Masking bright objects during iterative background refinement
- **Star detection**: Finding candidate pixels for connected-component labeling

## Architecture

```
threshold_mask/
├── mod.rs       # Public API and dispatch logic
├── sse.rs       # x86_64 SSE4.1 SIMD implementation
├── neon.rs      # ARM64 NEON SIMD implementation
├── tests.rs     # Unit tests (33 tests)
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

### Adaptive Per-Pixel Threshold
```rust
// pixel > background + adaptive_sigma[i] * noise
create_adaptive_threshold_mask(pixels, bg, noise, adaptive_sigma, mask);
```

Adaptive thresholding uses per-pixel sigma values computed from local contrast (coefficient of variation). Higher sigma in nebulous regions suppresses false detections; lower sigma in uniform sky enables faint star detection.

## Implementation Details

### Bit-Packed Storage

Uses `BitBuffer2` for memory efficiency - 1 bit per pixel instead of 1 byte (8x memory reduction). This improves cache utilization for large images.

### SIMD Strategy

The implementation processes 64 pixels per word, using 16 groups of 4 pixels each (matching SSE/NEON 128-bit register width).

```
Word (64 bits) = 16 groups × 4 pixels/group
                 ↓
         [group0][group1]...[group15]
            4b      4b         4b
```

**Per-group processing:**
1. Load 4 pixels, background, noise values
2. Compute threshold: `bg + sigma * max(noise, 1e-6)`
3. Compare: `pixels > threshold`
4. Extract comparison result as 4-bit mask
5. OR into word at appropriate bit position

### Platform-Specific Optimizations

| Platform | Implementation | Key Instructions |
|----------|----------------|------------------|
| x86_64   | SSE4.1         | `_mm_cmpgt_ps`, `_mm_movemask_ps` |
| ARM64    | NEON           | `vcgtq_f32`, `vgetq_lane_u32` |
| Other    | Scalar         | Bit-by-bit loop |

**SSE4.1 advantage**: `_mm_movemask_ps` directly extracts comparison results as a 4-bit integer, enabling efficient bit packing.

**NEON approach**: Extracts individual lane bits via `vgetq_lane_u32` since NEON lacks a direct movemask equivalent.

### Parallelization

Row-based parallel processing via Rayon. Each thread processes disjoint rows, writing to non-overlapping word ranges.

### Noise Floor Handling

Noise values are clamped to minimum `1e-6` to prevent division-by-zero-like artifacts when noise estimate is zero or negative.

## Performance

Benchmark results on 4K×4K image (16.7M pixels):

| Variant | SIMD | Scalar | Speedup |
|---------|------|--------|---------|
| Standard | 6.0ms | 19.3ms | 3.2× |
| Filtered | 4.3ms | 15.8ms | 3.7× |
| Adaptive | 7.8ms | 22.6ms | 2.9× |

Filtered is fastest (no background array access). Adaptive has ~30% overhead vs standard due to per-pixel sigma lookup.

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
