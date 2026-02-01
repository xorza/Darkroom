# Adaptive Local Thresholding Implementation Plan

## Problem Statement

The current detection pipeline uses a **single global sigma threshold** applied uniformly across the image:

```
threshold = background[x,y] + sigma_threshold × noise[x,y]
```

While the `BackgroundMap` already provides per-pixel background and noise estimates via tile interpolation, this approach fails in certain scenarios:

### Failure Cases

1. **Bright nebulosity**: Emission nebulae (H-alpha, O-III) significantly raise local surface brightness. Stars embedded in nebulae get lost because their pixel values don't exceed the elevated local threshold.

2. **Dust lanes / dark nebulae**: Absorption features lower the local signal. The interpolated background may not track rapid brightness changes, causing missed detections in transitional regions.

3. **Strong gradients**: Vignetting, flat-field residuals, or scattered moonlight create gradients that tile interpolation may smooth over too aggressively.

4. **Variable PSF across field**: Wide-field images have PSF variations that affect the effective noise per resolution element.

## Current Architecture

```
Image → TileGrid (64px tiles) → Bilinear interpolation → Per-pixel background/noise
                                                              ↓
                                           create_threshold_mask() with global sigma
                                                              ↓
                                                    Binary detection mask
```

### Key Components

- **TileGrid** (`tile_grid.rs`): Computes `TileStats { median, sigma }` per tile using sigma-clipped MAD
- **Interpolation** (`background/mod.rs`): Bilinear interpolation between tile centers
- **Threshold mask** (`common/threshold_mask/`): SIMD-accelerated thresholding with SSE4.1/NEON

## Proposed Solution: Tile-Adaptive Sigma Threshold

Instead of a global sigma value, use **per-tile sigma thresholds** that adapt to local conditions.

### Approach: Contrast-Based Adaptive Sigma

Adjust sigma threshold based on local signal characteristics:

```rust
struct AdaptiveTileStats {
    median: f32,           // Background level
    sigma: f32,            // Noise estimate (MAD-based)
    local_sigma: f32,      // Adaptive detection threshold (in sigma units)
}
```

**Algorithm:**

1. Compute base sigma threshold per tile based on local contrast:
   - **Low-contrast tiles** (uniform background): Use nominal sigma (e.g., 4.0)
   - **High-contrast tiles** (nebulosity, gradients): Increase sigma to reduce false positives
   - **Edge tiles** (transitions): Use intermediate values

2. Smooth adaptive sigma across tile grid to avoid discontinuities

3. Interpolate adaptive sigma to per-pixel values alongside background/noise

### Contrast Metric

Use the **coefficient of variation** (CV) or **robust scale estimate** of the tile:

```rust
// Option A: CV-based (simple)
let cv = tile_sigma / tile_median.abs().max(1e-6);
let adaptive_sigma = base_sigma + contrast_factor * cv;

// Option B: Percentile range (robust)
let p90 = percentile(tile_pixels, 0.90);
let p10 = percentile(tile_pixels, 0.10);
let range = (p90 - p10) / tile_sigma;
let adaptive_sigma = base_sigma * (1.0 + range_factor * range.max(0.0));
```

### Implementation Plan

#### Phase 1: Extend TileGrid (Low effort)

1. Add `local_sigma: f32` field to `TileStats`
2. Compute adaptive sigma during `fill_tile_stats()`:
   ```rust
   // After computing median and sigma
   let contrast = compute_contrast_metric(&samples, median, sigma);
   let local_sigma = compute_adaptive_sigma(base_sigma, contrast, &config);
   ```
3. Apply 3×3 median filter to adaptive sigma (already in place for median/sigma)

**Files to modify:**
- `star_detection/background/tile_grid.rs`

#### Phase 2: Extend Interpolation (Medium effort)

1. Add `adaptive_sigma: Buffer2<f32>` to `BackgroundMap`
2. Extend `interpolate_from_grid()` to interpolate three channels (background, noise, adaptive_sigma)
3. SIMD implementations already handle multiple values per iteration

**Files to modify:**
- `star_detection/background/mod.rs`
- `star_detection/background/simd/mod.rs` (minor: add third interpolation channel)

#### Phase 3: Update Threshold Mask (Low effort)

Create new function that uses per-pixel adaptive sigma:

```rust
pub fn create_adaptive_threshold_mask(
    pixels: &[f32],
    background: &[f32],
    noise: &[f32],
    adaptive_sigma: &[f32],  // Per-pixel sigma threshold
    mask: &mut BitBuffer2,
)
```

The existing SIMD structure can be reused with an additional load per iteration.

**Files to modify:**
- `star_detection/common/threshold_mask/mod.rs`
- `star_detection/common/threshold_mask/sse.rs`
- `star_detection/common/threshold_mask/neon.rs`

#### Phase 4: Configuration & API (Low effort)

Add configuration options:

```rust
pub struct AdaptiveThresholdConfig {
    /// Enable adaptive thresholding
    pub enabled: bool,
    
    /// Base sigma threshold (used in low-contrast regions)
    pub base_sigma: f32,  // default: 3.5
    
    /// Maximum sigma threshold (used in high-contrast regions)  
    pub max_sigma: f32,   // default: 6.0
    
    /// Contrast sensitivity factor
    pub contrast_factor: f32,  // default: 1.0
    
    /// Contrast metric: CV, PercentileRange, or MAD
    pub contrast_metric: ContrastMetric,
}
```

**Files to modify:**
- `star_detection/config.rs`

#### Phase 5: Integration (Low effort)

Update `detect_stars()` to use adaptive thresholding when enabled:

```rust
if config.adaptive_threshold.enabled {
    create_adaptive_threshold_mask(..., &background.adaptive_sigma, ...);
} else {
    create_threshold_mask(...);  // existing behavior
}
```

**Files to modify:**
- `star_detection/detection/mod.rs`

---

## Alternative Approaches Considered

### 1. Sliding Window Local Thresholding
- **Pros**: Finest granularity, best for complex fields
- **Cons**: O(N × W²) complexity, memory intensive, hard to SIMD
- **Verdict**: Too slow for real-time use

### 2. Multi-Resolution Threshold Maps
- **Pros**: Captures both large and small scale variations
- **Cons**: Complex implementation, multiple passes
- **Verdict**: Overkill for typical astronomical images

### 3. Machine Learning Thresholding
- **Pros**: Can learn complex patterns
- **Cons**: Training data needed, inference overhead, less interpretable
- **Verdict**: Out of scope for this project

### 4. Tile-Based (Proposed)
- **Pros**: Reuses existing infrastructure, minimal overhead, simple to understand
- **Cons**: Limited by tile resolution (64px default)
- **Verdict**: Best balance of complexity vs. benefit

---

## Performance Considerations

1. **Memory**: One additional `Buffer2<f32>` for adaptive sigma (~24MB for 6K image)
2. **Compute**: 
   - Tile stats: +1 division per tile (negligible)
   - Interpolation: +1 channel (already SIMD, ~30% more work)
   - Threshold mask: +1 load per 4/8 pixels (SIMD), ~20% slower
3. **Total overhead**: ~25-35% increase in threshold mask creation time (still <3ms for 6K)

---

## Testing Strategy

1. **Unit tests**:
   - Contrast metric computation
   - Adaptive sigma calculation edge cases
   - Interpolation of third channel

2. **Integration tests**:
   - Synthetic images with gradients
   - Synthetic nebula + star fields
   - Regression tests for uniform backgrounds

3. **Visual validation**:
   - Test on real images with known nebulosity
   - Compare detections with/without adaptive threshold

---

## Rollout Plan

1. **Phase 1-2**: Core implementation (tile stats + interpolation)
2. **Phase 3**: Threshold mask with SIMD
3. **Phase 4-5**: Configuration and integration
4. **Testing**: Comprehensive tests at each phase
5. **Benchmarking**: Verify performance impact

**Estimated effort**: ~2-3 sessions of focused work

---

## Success Criteria

1. Stars in nebulous regions detected with fewer false positives
2. No regression in detection quality for uniform backgrounds
3. Performance overhead < 50% for threshold mask creation
4. Configuration allows fine-tuning per use case
