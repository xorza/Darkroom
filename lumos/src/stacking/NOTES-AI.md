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
| `weighted.rs` | Weighted mean with quality-based frame weights |
| `rejection.rs` | Pixel rejection algorithms |
| `drizzle.rs` | Drizzle super-resolution stacking |

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

## Local Normalization (PLANNED)

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

### PixInsight-Style Algorithm

1. **Tile Division**: Divide image into tiles (typical: 128×128 to 256×256 pixels)
2. **Per-Tile Statistics**: Compute median and scale (MAD or std) for each tile
3. **Reference Frame**: Select or designate a reference frame
4. **Compute Correction Factors**:
   - For each tile: `offset = ref_median - target_median`
   - For each tile: `scale = ref_scale / target_scale`
5. **Smooth Interpolation**: Bilinearly interpolate between tile centers
6. **Apply Correction**: `pixel_corrected = (pixel - target_median) * scale + ref_median`

### Design Decisions

**Tile Size**:
- Default: 128×128 (matches existing background estimation)
- Configurable: 64-256 recommended range
- Smaller tiles = better gradient handling, but may introduce noise

**Statistics Method**:
- Use sigma-clipped median and MAD (robust to stars)
- Can reuse `sigma_clipped_stats()` from `background/mod.rs`

**Interpolation**:
- Bilinear interpolation between tile centers
- Can reuse SIMD interpolation from `background/simd/`

**API Design**:
```rust
/// Normalization method for aligning frame statistics before stacking.
#[derive(Debug, Clone)]
pub enum NormalizationMethod {
    /// No normalization.
    None,
    /// Global normalization - match overall median and scale.
    Global,
    /// Local normalization - tile-based matching.
    Local(LocalNormalizationConfig),
}

#[derive(Debug, Clone)]
pub struct LocalNormalizationConfig {
    /// Tile size in pixels (default: 128)
    pub tile_size: usize,
    /// Sigma for clipping outliers in tiles (default: 3.0)
    pub clip_sigma: f32,
    /// Number of clipping iterations (default: 3)
    pub clip_iterations: usize,
}
```

### Implementation Plan

1. Add `LocalNormalizationConfig` struct
2. Add `NormalizationMethod` enum with `None`, `Global`, `Local` variants
3. Create `local_normalization.rs` module:
   - `LocalNormalizationMap` struct (per-tile offset and scale)
   - `compute_local_normalization()` - compute tile statistics
   - `apply_local_normalization()` - apply with bilinear interpolation
4. Integrate with `WeightedConfig` to normalize before rejection
5. Unit tests with synthetic gradient data

### Key Insight: Reuse Background Estimation

The existing `star_detection/background/mod.rs` has:
- Tile-based sigma-clipped statistics (`TileStats`, `compute_tile_stats()`)
- Bilinear interpolation between tiles (`interpolate_row()`)
- SIMD-accelerated interpolation (`simd::interpolate_segment_simd()`)

Local normalization can share this infrastructure, computing:
- `TileStats { median, sigma }` - already exists
- `NormalizationTile { offset, scale }` - new, derived from reference vs target

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
