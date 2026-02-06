# Stacking Module

Image stacking for astrophotography with pixel rejection, frame weighting, normalization, and memory-mapped caching.

## Module Structure

| File | Description |
|------|-------------|
| `mod.rs` | Public API exports, `FrameType` enum |
| `config.rs` | `StackConfig`, `CombineMethod`, `Rejection`, `Normalization` |
| `stack.rs` | `stack()` / `stack_with_progress()` entry points, dispatch, normalization |
| `rejection.rs` | Pixel rejection algorithms |
| `cache.rs` | `ImageCache` — in-memory or disk-backed (mmap) storage |
| `cache_config.rs` | `CacheConfig` — adaptive chunk sizing (75% memory budget) |
| `progress.rs` | `ProgressCallback`, `StackingStage` |
| `error.rs` | `Error` enum |

## Public API

```rust
stack(paths, frame_type, config) -> Result<AstroImage, Error>
stack_with_progress(paths, frame_type, config, progress) -> Result<AstroImage, Error>

StackConfig        // { method, rejection, weights, normalization, cache }
CombineMethod      // Mean | Median | WeightedMean
Rejection          // None | SigmaClip | SigmaClipAsymmetric | Winsorized | LinearFit | Percentile | Gesd
Normalization      // None | Global
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

// Global normalization + custom rejection
let config = StackConfig {
    rejection: Rejection::SigmaClipAsymmetric {
        sigma_low: 4.0,
        sigma_high: 3.0,
        iterations: 5,
    },
    normalization: Normalization::Global,
    ..Default::default()
};
let result = stack(&paths, FrameType::Light, config)?;
```

## Rejection Algorithms

| Algorithm | Variant | Best For | Min Frames |
|-----------|---------|----------|------------|
| Sigma Clipping | `SigmaClip` | General use | 10+ |
| Asymmetric Sigma | `SigmaClipAsymmetric` | Bright outliers (satellites) | 10+ |
| Winsorized Sigma | `Winsorized` | Small stacks, data preservation | 5+ |
| Linear Fit | `LinearFit` | Sky gradients, temporal trends | 15+ |
| Percentile Clipping | `Percentile` | Very small stacks | 3+ |
| GESD | `Gesd` | Large stacks, rigorous detection | 50+ |

### Sigma Thresholds

- **2.0-2.5**: Aggressive — removes satellites, airplane trails
- **2.5-3.0**: Standard (recommended default)
- **3.0-4.0**: Conservative — preserves more data
- Never below 2.0 — causes excessive false rejection

### Calibration Frame Recommendations

| Frame Type | Preset | Notes |
|------------|--------|-------|
| Bias | `StackConfig::mean()` | No outliers expected |
| Dark | `StackConfig::sigma_clipped(3.0)` | Remove cosmic rays |
| Flat | `StackConfig::sigma_clipped(2.5)` | Remove dust artifacts |
| Light | `StackConfig::default()` | General purpose |

## Global Normalization

When `Normalization::Global` is set, each frame is affine-transformed before combining to match the reference frame (frame 0):

```
normalized = pixel * gain + offset
gain   = ref_MAD / frame_MAD
offset = ref_median - frame_median * gain
```

Per-frame, per-channel median and MAD (robust scale) are computed from the full frame data. This corrects brightness and contrast differences between frames from different sessions, changing sky conditions, or sensor temperature drift.

## Key Design Decisions

- **MAD for scale estimation**: All sigma clipping (symmetric, asymmetric, winsorized) uses MAD (Median Absolute Deviation) instead of stddev. MAD is robust to the outliers being rejected — stddev is inflated by them, leading to under-rejection. Matches Siril and PixInsight behavior.
- **Asymmetric sigma clipping**: Separate algorithm from linear fit clipping. Uses median as center with independent low/high thresholds. Default: sigma_low=4.0, sigma_high=3.0 (PixInsight convention).
- **Weighted rejection**: Percentile clipping sorts (value, weight) pairs together. Winsorized applies winsorization first, then weighted mean. Other rejection methods reject from unweighted statistics, then apply weighted mean to survivors.
- **No local normalization**: Tile-based PixInsight-style local normalization was evaluated and removed — niche benefit for most workflows, significant complexity.
- **No auto frame weighting**: Evaluated and removed — marginal benefit vs. manual weights or equal weighting with good rejection.

## Architecture

### Storage

`ImageCache` auto-selects storage mode based on available memory:
- **In-memory**: When all frames fit in 75% of RAM. Stores `AstroImage` directly.
- **Disk-backed**: Writes per-channel binary f32 files, accessed via memory-mapped I/O. Enables stacking 100+ frames of 50MP images.

### Processing Pipeline

1. **Load**: Decode images, cache to disk or memory (parallelized via rayon)
2. **Normalize** (optional): Compute per-frame median/MAD, derive affine parameters
3. **Process**: For each pixel position across all frames — apply normalization, reject outliers, combine
4. **Cleanup**: Remove cache files (unless `keep_cache = true`)

Processing is chunked by rows and parallelized per-row with rayon. Each channel is processed independently for efficient planar data access.
