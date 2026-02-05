# Stacking Module

Image stacking algorithms for astrophotography, including mean, median, sigma-clipped, weighted mean, and pixel rejection methods.

## Module Structure

| Module | Description |
|--------|-------------|
| `mod.rs` | `StackingMethod`, `FrameType`, `ImageStack` orchestrator |
| `error.rs` | `Error` enum for stacking operations |
| `cache.rs` | `ImageCache` with memory-mapped binary cache |
| `cache_config.rs` | `CacheConfig` with adaptive chunk sizing |
| `mean/` | Mean stacking |
| `median/` | Median stacking via mmap |
| `sigma_clipped/` | Sigma-clipped mean via mmap |
| `weighted/` | Weighted mean with quality-based frame weights |
| `rejection.rs` | Pixel rejection algorithms |
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
NormalizationMethod // None | Global | Local(LocalNormalizationConfig)
LocalNormalizationConfig // { tile_size, clip_sigma, clip_iterations }
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

## Usage Examples

### Basic Stacking

```rust
use lumos::stacking::{ImageStack, FrameType, StackingMethod, ProgressCallback};

// Mean stacking (fastest, no outlier rejection)
let stack = ImageStack::new(FrameType::Dark, StackingMethod::Mean, &paths);
let result = stack.process(ProgressCallback::default())?;

// Median stacking (default, good outlier rejection)
let stack = ImageStack::new(FrameType::Light, StackingMethod::default(), &paths);
let result = stack.process(ProgressCallback::default())?;

// Sigma-clipped mean
let config = SigmaClippedConfig::default(); // sigma=2.5, iterations=3
let stack = ImageStack::new(
    FrameType::Light,
    StackingMethod::SigmaClippedMean(config),
    &paths,
);
let result = stack.process(ProgressCallback::default())?;
```

### Weighted Stacking with Quality Metrics

```rust
use lumos::stacking::{WeightedConfig, FrameQuality, RejectionMethod};

// Compute quality from star detection results
let qualities: Vec<FrameQuality> = detection_results
    .iter()
    .map(FrameQuality::from_detection_result)
    .collect();

// Create weighted config
let config = WeightedConfig::from_quality(&qualities)
    .with_rejection(RejectionMethod::SigmaClip(SigmaClipConfig::default()));

let stack = ImageStack::new(
    FrameType::Light,
    StackingMethod::WeightedMean(config),
    &paths,
);
```

---

## Industry Best Practices

### Rejection Algorithm Selection by Stack Size

| Frame Count | Recommended Method | Rationale |
|-------------|-------------------|-----------|
| 3-9 | Percentile Clipping | Simple, robust with limited data |
| 10-15 | Winsorized Sigma Clip | Preserves data, more robust than basic sigma |
| 15-25 | Sigma Clipping (κ=2.5-3.0) | Good balance, enough data for statistics |
| 25-50 | Linear Fit Clipping | Handles sky gradients, more accurate |
| 50+ | GESD | Rigorous statistical test, best with large data |

### Sigma Threshold Guidelines

- **κ=2.0-2.5**: Aggressive (satellite trails, airplane lights)
- **κ=2.5-3.0**: Standard (recommended default)
- **κ=3.0-4.0**: Conservative (preserves more data)

### Calibration Frame Recommendations

| Frame Type | Recommended Method | Sigma | Notes |
|------------|-------------------|-------|-------|
| Bias | Mean | - | No outliers in bias frames |
| Dark | Sigma Clipping | 3.0 | Remove cosmic rays |
| Flat | Sigma Clipping or Median | 2.5-3.0 | Remove dust shadows |
| Light | SigmaClip/Winsorized/LinearFit | 2.5-3.0 | Choose based on frame count |

---

## Local Normalization

Corrects illumination differences across frames by matching brightness locally rather than globally. Handles vignetting, sky gradients, and session-to-session brightness variations.

### Algorithm

1. Divide image into tiles (default: 128×128 pixels)
2. Compute sigma-clipped median and MAD for each tile
3. Compare target frame tiles to reference frame tiles
4. Compute per-tile offset and scale correction factors
5. Bilinearly interpolate between tile centers
6. Apply: `pixel_corrected = (pixel - target_median) * scale + ref_median`

### Usage

```rust
use lumos::stacking::{LocalNormalizationConfig, compute_normalization_map};

let config = LocalNormalizationConfig::default();  // 128px tiles
// Or: LocalNormalizationConfig::fine()   // 64px tiles for steep gradients
// Or: LocalNormalizationConfig::coarse() // 256px tiles for stability

let map = compute_normalization_map(&reference, &target, &config);
map.apply(&mut target_pixels);
```

---

## Future Improvements

### High Priority

1. **Adaptive Rejection Selection**: Auto-select method based on frame count
2. **Asymmetric Sigma Clipping**: Separate thresholds for high/low outliers
3. **SIMD-Optimized Rejection**: Vectorize inner loops for 2-4x speedup

### Medium Priority

4. **Robust Scale Estimators**: Sn/Qn estimators for non-Gaussian data
5. **Stacking Presets**: Pre-configured settings for common scenarios

### Advanced

6. **Per-Pixel Noise Weighting**: Weight = 1/variance per pixel
7. **GPU Acceleration**: Compute shader for sigma clipping
8. **Live Stacking**: Real-time preview during capture
9. **Multi-Session Integration**: Cross-session normalization and weighting
