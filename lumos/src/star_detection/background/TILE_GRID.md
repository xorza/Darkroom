# TileGrid: Tile-Based Background Statistics

`TileGrid` computes robust statistics (median, sigma) for image tiles, forming the foundation for background estimation.

## Algorithm Overview

```
Input Image (W×H pixels)
         │
         ▼
┌─────────────────────────────────┐
│  Divide into tiles (tile_size)  │
│  e.g., 6144×6144 → 96×96 tiles  │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│  Per-tile sigma-clipped stats   │
│  • Collect pixels (sampled)     │
│  • 3σ clipping, 2 iterations    │
│  • Median + MAD-based sigma     │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│  3×3 median filter on tiles     │
│  • Suppresses outlier tiles     │
│  • Reduces interpolation rings  │
└─────────────────────────────────┘
         │
         ▼
    TileGrid ready for interpolation
```

## Sigma-Clipped Statistics

### Why Sigma Clipping?

Raw statistics are biased by astronomical sources:
- Mean is skewed by bright stars
- Standard deviation is inflated
- Background estimate is too high

Sigma clipping iteratively removes outliers:

```
Initial: [100, 100, 100, 100, 5000]  ← star pixel
Median:  100
MAD:     0 → σ = 0
Clip:    Remove 5000 (> median + 3σ)
Final:   median=100, σ≈0 (correct background)
```

### Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Sigma | 3.0 | Standard; rejects 0.27% of Gaussian |
| Iterations | 2 (configurable) | MAD converges quickly; configurable via `sigma_clip_iterations` |
| Center | Median | Robust to outliers |
| Scale | MAD × 1.4826 | Robust σ estimator |

The number of sigma clipping iterations is configurable via `BackgroundConfig::sigma_clip_iterations`. With MAD-based sigma estimation, 2-3 iterations typically suffice (vs. 5-10 with standard deviation).

### MAD-Based Sigma

The Median Absolute Deviation is more robust than standard deviation:

```
MAD = median(|x_i - median(x)|)
σ_MAD = MAD × 1.4826
```

The factor 1.4826 makes MAD consistent with σ for Gaussian distributions.

## Sampling Strategy

For large tiles, computing statistics on all pixels is wasteful:

| Tile Size | Pixels | Sampled | Speedup |
|-----------|--------|---------|---------|
| 32×32 | 1,024 | 1,024 | 1× |
| 64×64 | 4,096 | ~1,024 | 4× |
| 128×128 | 16,384 | ~1,024 | 16× |

Sampling uses 2D strided access to preserve spatial distribution:

```rust
let stride = sqrt(tile_pixels / MAX_TILE_SAMPLES);
for y in (y_start..y_end).step_by(stride) {
    for x in (x_start..x_end).step_by(stride) {
        values.push(pixels[y * width + x]);
    }
}
```

With ~1000 samples, median accuracy is within 1-2%.

## Source Masking

When a source mask is provided:

1. **Collect unmasked pixels** using word-level bit operations
2. **Fallback** to all pixels if too few unmasked (< min_pixels)
3. **Subsample** if too many unmasked (> MAX_TILE_SAMPLES)

Word-level operations process 64 mask bits at a time:

```rust
let unmasked = !mask_word & relevant_bits;
while bits != 0 {
    let offset = bits.trailing_zeros();
    values.push(pixels[row_start + offset]);
    bits &= bits - 1;  // Clear lowest bit
}
```

## 3×3 Median Filter

After computing tile statistics, a median filter suppresses outliers:

```
Before:  [50] [50] [50]      After:  [50] [50] [50]
         [50][200] [50]   →          [50] [50] [50]
         [50] [50] [50]              [50] [50] [50]
```

Benefits:
- Removes tiles contaminated by bright stars
- Reduces bicubic spline ringing (SExtractor approach)
- Smooths tile-to-tile variations

The filter is skipped for grids smaller than 3×3.

## Tile Center Computation

Each tile has a center used for interpolation:

```
┌─────────────────────────────────┐
│  Tile (tx=1)                    │
│  x_start = 64, x_end = 128      │
│  center_x = (64 + 128) / 2 = 96 │
└─────────────────────────────────┘
```

Partial tiles (at image edges) have adjusted centers:

```
Image width = 100, tile_size = 64
Tile 0: x=[0,64),   center=32
Tile 1: x=[64,100), center=82  (not 96!)
```

## Binary Search for Tile Lookup

`find_lower_tile_y(pos)` finds the tile whose center is at or before `pos`:

```rust
// Binary search: O(log n) instead of O(n)
let mut lo = 0;
let mut hi = tiles_y;
while lo < hi {
    let mid = lo + (hi - lo) / 2;
    if center_y(mid) <= pos {
        lo = mid + 1;
    } else {
        hi = mid;
    }
}
lo.saturating_sub(1)
```

## Comparison with Reference Implementations

### SExtractor

| Feature | SExtractor | TileGrid |
|---------|------------|----------|
| Clipping | ±3σ | ±3σ |
| Center | Median → Mode* | Median |
| Scale | Std Dev | MAD × 1.4826 |
| Filter | 3×3 median | 3×3 median |
| Interpolation | Bicubic spline | Bilinear |

*SExtractor uses Mode = 2.5×Median - 1.5×Mean for crowded fields

### Photutils

| Feature | Photutils | TileGrid |
|---------|-----------|----------|
| Clipping | sigma=3, maxiters=10 | sigma=3, iters=2 |
| Estimators | Pluggable | Median + MAD |
| Filter | Configurable | 3×3 fixed |
| Interpolation | Zoom (spline) or IDW | Bilinear |

## Performance

Benchmarks on 6144×6144 globular cluster (50,000 stars):

| Configuration | Time |
|---------------|------|
| No mask | 45ms |
| With mask (6.8% masked) | 32ms |

The masked case is faster because:
1. Fewer pixels to collect (masked regions skipped)
2. Word-level bit operations are efficient
3. Subsampling kicks in for dense unmasked regions

## Test Coverage

48 tests covering:
- Dimension calculations
- Center computations
- Binary search correctness
- Mask handling (none, partial, full)
- Median filter behavior
- Sigma clipping outlier rejection
- MAD-based sigma accuracy
- Gradient preservation
- Sparse star rejection
- Edge cases (single tile, very wide/tall images)

## References

- [SExtractor Background](https://sextractor.readthedocs.io/en/latest/Background.html): Mesh-based estimation with κσ clipping
- [Photutils Background2D](https://photutils.readthedocs.io/en/stable/user_guide/background.html): Python implementation
- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html): Sigma clipping algorithm
