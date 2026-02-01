# Background Estimation Module

This module implements tile-based background and noise estimation for astronomical images, following best practices from [SExtractor](https://sextractor.readthedocs.io/en/latest/Background.html) and [Photutils](https://photutils.readthedocs.io/en/stable/user_guide/background.html).

## Overview

Background estimation is critical for accurate star detection. The sky background in astronomical images varies spatially due to:
- Vignetting (optical falloff toward edges)
- Light pollution gradients
- Scattered moonlight
- Thermal emission (infrared)

This module provides robust background estimation that:
1. Handles spatial variations through tile-based sampling
2. Rejects stars and artifacts using sigma-clipped statistics
3. Creates smooth background maps via bilinear interpolation
4. Supports iterative refinement with source masking

## Architecture

```
BackgroundMap::new(pixels, config)
        │
        ▼
┌──────────────────┐
│    TileGrid      │  ◄── Divides image into tiles, computes statistics
│  (tile_grid.rs)  │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│   Interpolation  │  ◄── Bilinear interpolation between tile centers
│    (mod.rs)      │
└────────┬─────────┘
         │
         ▼ (if iterations > 0)
┌──────────────────┐
│    Refinement    │  ◄── Mask detected sources, re-estimate
│    (mod.rs)      │
└──────────────────┘
```

## Algorithm

### Phase 1: Tile Statistics (TileGrid)

The image is divided into a grid of tiles (default 64×64 pixels). For each tile:

1. **Pixel Collection**: Gather pixel values (with optional source mask)
2. **Sigma Clipping**: Iteratively reject outliers beyond 3σ from median
3. **Statistics**: Compute median (background) and MAD-based sigma (noise)
4. **Median Filter**: Apply 3×3 median filter to tile statistics

This follows SExtractor's approach where the background estimator uses "κσ clipping and mode estimation" with "iterative clipping at ±3σ around the median."

### Phase 2: Interpolation

Bilinear interpolation creates smooth per-pixel background and noise maps:

```
For each pixel (x, y):
    1. Find surrounding tile centers
    2. Compute interpolation weights
    3. Blend tile statistics bilinearly
```

SExtractor uses bicubic spline interpolation, but bilinear is faster and sufficient for most use cases. The 3×3 median filter on tiles reduces artifacts.

### Phase 3: Iterative Refinement (Optional)

For crowded fields, iterative refinement improves accuracy:

1. Detect pixels above `sigma_threshold × noise`
2. Dilate mask by `mask_dilation` pixels
3. Re-estimate background excluding masked pixels
4. Repeat for `iterations` cycles

This is similar to Photutils' recommendation to use source masks for accurate background estimation.

## Configuration

```rust
BackgroundConfig {
    tile_size: 64,              // Tile size in pixels (32-512 typical)
    iterations: 1,              // Refinement iterations (0 = none)
    sigma_threshold: 3.0,       // Detection threshold for masking
    mask_dilation: 5,           // Mask dilation radius
    min_unmasked_fraction: 0.1, // Min unmasked pixels per tile
    sigma_clip_iterations: 2,   // Sigma clipping iterations (2-5 typical)
}
```

### Choosing Tile Size

From SExtractor documentation:
> "The choice of the mesh size is very important. If it is too small, the background estimation is affected by the presence of objects and random noise. If the mesh size is too large, it cannot reproduce the small scale variations of the background."

Guidelines:
- **32-64 pixels**: Fine-grained, good for strong gradients
- **64-128 pixels**: Balanced (recommended default)
- **128-256 pixels**: Smooth, less sensitive to sources

## Implementation Details

### Sigma Clipping

Uses median and MAD (Median Absolute Deviation) for robustness:
- Median is resistant to outliers (unlike mean)
- MAD-based sigma: `σ = MAD × 1.4826`
- 3σ clipping rejects ~0.3% of Gaussian data per iteration
- 2 iterations sufficient with MAD (converges quickly)

### Sampling Optimization

Large tiles use sampling (~1024 pixels) instead of all pixels:
- Median estimate accuracy: ~1% with 1000 samples
- Significant speedup for large tile sizes
- Strided sampling preserves spatial distribution

### SIMD Acceleration

Row interpolation uses SIMD for throughput:
- Processes 8 pixels per iteration (AVX/NEON)
- ~3-4× speedup over scalar code

## Performance

Benchmark results (6144×6144 globular cluster image):

| Operation | Time |
|-----------|------|
| TileGrid (no mask) | 45ms |
| TileGrid (with mask) | 32ms |
| Full BackgroundMap | ~100ms |

## References

- [SExtractor Background Modeling](https://sextractor.readthedocs.io/en/latest/Background.html)
- [Photutils Background2D](https://photutils.readthedocs.io/en/stable/user_guide/background.html)
- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
- Bertin & Arnouts (1996), "SExtractor: Software for source extraction"

## Files

- `mod.rs` - BackgroundMap, interpolation, refinement
- `tile_grid.rs` - TileGrid, sigma-clipped statistics
- `simd.rs` - SIMD-accelerated interpolation
- `bench.rs` - Benchmarks
- `tests.rs` - Integration tests
