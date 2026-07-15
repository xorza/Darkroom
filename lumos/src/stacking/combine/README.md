# Stacking Module

Image stacking for astrophotography with pixel rejection, frame weighting, normalization, and memory-mapped caching.

## Module Structure

| File | Description |
|------|-------------|
| `mod.rs` | Public API exports |
| `config.rs` | `StackConfig`, `CombineMethod`, `Rejection`, `Normalization` |
| `stack.rs` | `stack()` (from paths) / `stack_images()` (in-memory) entry points, dispatch, normalization |
| `rejection.rs` | Pixel rejection algorithms |
| `../frame_store/mod.rs` | Shared memory planning, RAM/mmap planes, spill cleanup, stored frames |
| `cache/mod.rs` | Chunked combine engine and product quality planes |
| `cache/loader/mod.rs` | Tier selection, frame loading, and cache sidecars |
| `cache/tests.rs` | Combine-engine tests and shared stack test helper |
| `cache/loader/tests.rs` | Loader and sidecar tests |
| `cache_config.rs` | `CacheConfig` and available-memory query |
| `../progress.rs` | Shared `ProgressCallback`, `StackingProgress`, and `StackingStage` |
| `error.rs` | `Error` enum |

## Public API

```rust
stack(paths, config, progress, cancel)         -> Result<StackProduct, Error>
stack_images(frames, config, progress, cancel) -> Result<StackProduct, Error>

StackConfig        // { method, weighting, normalization, cache }
CombineMethod      // Mean(Rejection) | Median
Rejection          // None | SigmaClip | SigmaClipAsymmetric | Winsorized | LinearFit | Percentile | Gesd
Normalization      // None | Global | Multiplicative
CacheConfig        // { cache_dir, keep_cache, available_memory }
ProgressCallback   // pass ::default() for no progress reporting
```

## Usage

```rust
use lumos::{stack, Normalization, ProgressCallback, Rejection, StackConfig};

// Every entry point takes a ProgressCallback; ::default() reports nothing.
let none = ProgressCallback::default();

// Default: sigma-clipped mean (sigma=2.5, 3 iterations) + presets
let result = stack(&paths, StackConfig::default(), none.clone())?;
let result = stack(&paths, StackConfig::median(), none.clone())?;
let result = stack(&paths, StackConfig::winsorized(3.0), none.clone())?;

// Weighted stacking
let result = stack(&paths, StackConfig::weighted(vec![1.0, 0.8, 1.2]), none.clone())?;

// Global normalization + custom rejection
let config = StackConfig {
    rejection: Rejection::sigma_clip_asymmetric(4.0, 3.0),
    normalization: Normalization::Global,
    ..Default::default()
};
let result = stack(&paths, config, none)?;
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
- **Weighted rejection**: Percentile clipping sorts values with an index co-array for weight lookup, then computes weighted mean over the surviving range. Winsorized applies winsorization first, then weighted mean. Other rejection methods reject from unweighted statistics, then apply weighted mean to survivors via index mapping.
- **No local normalization**: Tile-based PixInsight-style local normalization was evaluated and removed — niche benefit for most workflows, significant complexity.
- **No auto frame weighting**: Evaluated and removed — marginal benefit vs. manual weights or equal weighting with good rejection.

## Architecture

### Storage

`stacking::frame_store` owns the storage representation and memory arithmetic shared by combine and
the end-to-end pipeline:

- **In-memory**: channel and coverage planes use resident `Buffer2<f32>` storage.
- **Disk-backed**: channel and coverage planes use memory-mapped f32 files owned by a
  `SpillDirectory`; dropping the final cache releases mappings before cleanup.
- `StoredLightFrame` keeps channels, optional coverage, and per-channel median/MAD statistics in one
  record. Combine consumes these records without exposing its cache internals to pipeline.

### Processing Pipeline

1. **Load**: Decode images, cache to disk or memory (parallelized via rayon)
2. **Normalize** (optional): Compute per-frame median/MAD, derive affine parameters
3. **Process**: For each pixel position across all frames — apply normalization, reject outliers, combine
4. **Cleanup**: Remove cache files (unless `keep_cache = true`)

Processing is chunked by rows and parallelized per-row with rayon. Each channel is processed independently for efficient planar data access.
