# Stacking Module - Implementation Notes (AI)

## Overview

Image stacking algorithms for astrophotography, including mean, median, sigma-clipped, weighted mean, drizzle super-resolution, and pixel rejection methods.

## Module Structure

| Module | Description |
|--------|-------------|
| `mod.rs` | `StackingMethod`, `FrameType`, `ImageStack`, dispatch |
| `error.rs` | `Error` enum for stacking operations |
| `cache.rs` | `ImageCache` with memory-mapped binary cache |
| `cache_config.rs` | `CacheConfig` with adaptive chunk sizing |
| `mean/` | Mean stacking (SIMD: NEON/SSE/scalar) |
| `median/` | Median stacking via mmap (SIMD sorting networks) |
| `sigma_clipped/` | Sigma-clipped mean via mmap |
| `weighted/` | Weighted mean with quality-based frame weights |
| `rejection.rs` | Pixel rejection algorithms |
| `drizzle.rs` | Drizzle super-resolution stacking |
| `local_normalization.rs` | Local normalization (tile-based, PixInsight-style) |

## Key Types

```rust
StackingMethod     // Mean | Median | SigmaClippedMean | WeightedMean
FrameType          // Dark | Flat | Bias | Light
ImageStack         // Main stacking orchestrator
CacheConfig        // { cache_dir, keep_cache, available_memory }
WeightedConfig     // { weights, rejection, cache }
FrameQuality       // { snr, fwhm, eccentricity, noise, star_count }
RejectionMethod    // None | SigmaClip | WinsorizedSigmaClip | LinearFitClip | PercentileClip | Gesd
DrizzleConfig      // { scale, pixfrac, kernel, min_coverage, fill_value }
DrizzleKernel      // Square | Point | Gaussian | Lanczos
NormalizationMethod // None | Global | Local(LocalNormalizationConfig)
LocalNormalizationConfig // { tile_size, clip_sigma, clip_iterations }
TileNormalizationStats   // Per-tile median and scale statistics
LocalNormalizationMap    // Correction map for applying normalization
```

## Rejection Methods

| Method | Best For | Description |
|--------|----------|-------------|
| SigmaClip | General use | Iterative kappa-sigma clipping |
| WinsorizedSigmaClip | Preserving data | Replace outliers with boundary values |
| LinearFitClip | Sky gradients | Fits line to pixel stack, rejects deviants |
| PercentileClip | Small stacks | Simple low/high percentile rejection |
| GESD | Large stacks (>50) | Generalized Extreme Studentized Deviate Test |

---

## Local Normalization (IMPLEMENTED)

### Research Summary

**Sources:**
- [PixInsight Local Normalization](https://chaoticnebula.com/pixinsight-local-normalization/)
- [Astro Pixel Processor LNC FAQ](https://www.astropixelprocessor.com/community/faq/what-exactly-does-the-lnc-and-when-do-i-change-the-default-values-to-something-else/)
- [Siril Stacking Documentation](https://siril.readthedocs.io/en/stable/preprocessing/stacking.html)

### Algorithm Overview

Local Normalization corrects illumination differences across frames by matching brightness **locally** rather than globally. This handles:
- Vignetting (darker corners, brighter center)
- Sky gradients (light pollution, moon, twilight)
- Session-to-session brightness variations

**Key benefit**: Dramatically improves pixel rejection in final integration by ensuring all subframes have matched, flat backgrounds.

### Implementation Details

**Module**: `local_normalization.rs`

**Algorithm Steps**:
1. **Tile Division**: Divide image into tiles (default: 128×128, configurable 64-256)
2. **Per-Tile Statistics**: Compute sigma-clipped median and MAD for each tile (reuses `sigma_clipped_median_mad()` from `star_detection/constants`)
3. **Compute Correction Factors**:
   - `offset = ref_median` (per tile)
   - `scale = ref_scale / target_scale` (clamped to avoid division by near-zero)
   - `target_median` stored for the correction formula
4. **Smooth Interpolation**: Bilinear interpolation between tile centers (segment-based for efficiency)
5. **Apply Correction**: `pixel_corrected = (pixel - target_median) * scale + ref_median`

**Key Types**:
```rust
// Normalization method enum
pub enum NormalizationMethod {
    None,              // No normalization
    Global,            // Match overall median and scale (default)
    Local(LocalNormalizationConfig), // Tile-based matching
}

// Configuration
pub struct LocalNormalizationConfig {
    pub tile_size: usize,        // Default: 128, Range: 64-256
    pub clip_sigma: f32,         // Default: 3.0
    pub clip_iterations: usize,  // Default: 3
}

// Presets
LocalNormalizationConfig::fine()   // 64px tiles - steep gradients
LocalNormalizationConfig::coarse() // 256px tiles - stability

// Per-tile statistics
pub struct TileNormalizationStats {
    medians: Vec<f32>,
    scales: Vec<f32>,
    tiles_x, tiles_y, tile_size, width, height
}

// Correction map
pub struct LocalNormalizationMap {
    offsets, scales, target_medians: Vec<f32>,
    centers_x, centers_y: Vec<f32>,
    tiles_x, tiles_y, width, height
}
```

**Convenience Functions**:
```rust
// Compute normalization map from reference and target
compute_normalization_map(reference, target, width, height, config) -> LocalNormalizationMap

// Apply normalization in one step
normalize_frame(reference, target, width, height, config) -> Vec<f32>

// Apply to image in-place
map.apply(&mut pixels)

// Apply returning new image
map.apply_to_new(&pixels) -> Vec<f32>
```

**Performance**:
- Parallel tile statistics computation via rayon
- Row-based parallel processing for apply
- Segment-based interpolation (amortizes tile lookups)

**Test Coverage** (25 tests):
- Config validation and presets
- Uniform/gradient image statistics
- Offset and gradient correction
- Single tile and non-multiple-of-tile-size images
- In-place vs new apply consistency

---

## Global Normalization (Current)

The current implementation uses global statistics:
- Compute overall median and scale for reference frame
- Compute overall median and scale for each target frame
- Apply uniform offset and scale to entire frame

This works well for single-session data with minimal gradients but fails for:
- Multi-session data with different sky conditions
- Frames with varying light pollution gradients
- Mosaics with different overlap regions
