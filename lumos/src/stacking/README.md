# Stacking Module

Image stacking for astrophotography with pixel rejection, frame weighting, normalization, and memory-mapped caching.

## Module Structure

| File | Description |
|------|-------------|
| `mod.rs` | Public API exports, `FrameType` enum |
| `config.rs` | Unified `StackConfig`, `CombineMethod`, `Rejection`, `Normalization` |
| `stack.rs` | `stack()` and `stack_with_progress()` entry points |
| `rejection.rs` | Pixel rejection algorithms (sigma clip, winsorized, linear fit, percentile, GESD) |
| `local_normalization.rs` | Tile-based local normalization (PixInsight-style) |
| `cache.rs` | `ImageCache` with memory-mapped binary cache |
| `cache_config.rs` | `CacheConfig` with adaptive chunk sizing (75% memory budget) |
| `progress.rs` | `ProgressCallback`, `StackingStage` |
| `error.rs` | `Error` enum for stacking operations |

## Public API

```rust
// Entry points
stack(paths, frame_type, config) -> Result<AstroImage, Error>
stack_with_progress(paths, frame_type, config, progress) -> Result<AstroImage, Error>

// Configuration
StackConfig        // { method, rejection, weights, normalization, cache }
CombineMethod      // Mean | Median | WeightedMean
Rejection          // None | SigmaClip | SigmaClipAsymmetric | Winsorized | LinearFit | Percentile | Gesd
Normalization      // None | Global | Local { tile_size }
FrameType          // Dark | Flat | Bias | Light
CacheConfig        // { cache_dir, keep_cache, available_memory }
```

## Usage

```rust
use lumos::stacking::{stack, StackConfig, FrameType, Rejection, Normalization};

// Default: sigma-clipped mean (sigma=2.5, 3 iterations)
let result = stack(&paths, FrameType::Light, StackConfig::default())?;

// Presets
let result = stack(&paths, FrameType::Light, StackConfig::median())?;
let result = stack(&paths, FrameType::Light, StackConfig::sigma_clipped(2.0))?;
let result = stack(&paths, FrameType::Bias, StackConfig::mean())?;
let result = stack(&paths, FrameType::Light, StackConfig::winsorized(3.0))?;
let result = stack(&paths, FrameType::Light, StackConfig::linear_fit(2.5))?;
let result = stack(&paths, FrameType::Light, StackConfig::percentile(15.0))?;
let result = stack(&paths, FrameType::Light, StackConfig::gesd())?;

// Weighted stacking
let result = stack(&paths, FrameType::Light, StackConfig::weighted(vec![1.0, 0.8, 1.2]))?;

// Custom configuration with struct update syntax
let config = StackConfig {
    rejection: Rejection::SigmaClipAsymmetric {
        sigma_low: 4.0,
        sigma_high: 3.0,
        iterations: 5,
    },
    normalization: Normalization::Local { tile_size: 128 },
    ..Default::default()
};
let result = stack(&paths, FrameType::Light, config)?;
```

## Rejection Algorithms

| Algorithm | Enum Variant | Best For | Min Frames |
|-----------|-------------|----------|------------|
| Sigma Clipping | `SigmaClip` | General use | 10+ |
| Asymmetric Sigma | `SigmaClipAsymmetric` | Bright outliers (satellites) | 10+ |
| Winsorized Sigma | `Winsorized` | Small stacks, data preservation | 5+ |
| Linear Fit | `LinearFit` | Sky gradients, temporal trends | 15+ |
| Percentile Clipping | `Percentile` | Very small stacks | 3+ |
| GESD | `Gesd` | Large stacks, rigorous detection | 50+ |

### Sigma Threshold Guidelines

- **2.0-2.5**: Aggressive (removes satellites, airplane trails)
- **2.5-3.0**: Standard (recommended default)
- **3.0-4.0**: Conservative (preserves more data, less false rejection)
- Never below 2.0 (causes excessive false rejection)

### Calibration Frame Recommendations

| Frame Type | Method | Rejection | Notes |
|------------|--------|-----------|-------|
| Bias | `StackConfig::mean()` | None | No outliers expected |
| Dark | `StackConfig::sigma_clipped(3.0)` | SigmaClip | Remove cosmic rays |
| Flat | `StackConfig::sigma_clipped(2.5)` | SigmaClip | Remove dust artifacts |
| Light | `StackConfig::default()` | SigmaClip(2.5) | General purpose |

## Local Normalization

Tile-based correction for vignetting, sky gradients, and session-to-session brightness variations (PixInsight-style). Fully implemented but not yet integrated into the stacking pipeline.

### Algorithm

1. Divide image into tiles (default 128x128)
2. Compute sigma-clipped median and MAD per tile
3. Compare target tiles to reference tiles
4. Compute per-tile offset and scale factors
5. Bilinear interpolation between tile centers
6. Apply: `corrected = (pixel - target_median) * scale + ref_median`

## Architecture

### Memory Management

Images are decoded and cached to disk as binary f32 files via memory-mapped I/O. The `CacheConfig` controls adaptive chunk sizing: uses 75% of available RAM, with a minimum of 64 rows per chunk. This enables stacking large datasets (100+ frames of 50MP images) without exceeding memory limits.

### Processing Pipeline

1. **Load**: Decode images, write to disk cache (parallelized)
2. **Process**: Read pixel columns across all frames via mmap, apply rejection + combine per-pixel
3. **Cleanup**: Remove cache files (unless `keep_cache = true`)

Rejection and combination operate on pixel stacks: for each output pixel position, the module collects the value from all input frames, applies the configured rejection to remove outliers, then combines the remaining values.
