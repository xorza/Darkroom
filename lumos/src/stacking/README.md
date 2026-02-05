# Stacking Module

Image stacking algorithms for astrophotography, including mean, median, sigma-clipped, weighted mean, drizzle super-resolution, and pixel rejection methods.

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
| `drizzle.rs` | Drizzle super-resolution stacking |
| `local_normalization.rs` | Local normalization (tile-based, PixInsight-style) |
| `gradient_removal.rs` | Post-stack gradient removal (polynomial/RBF) |

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

### Drizzle Super-Resolution

```rust
use lumos::stacking::{DrizzleConfig, DrizzleKernel, drizzle_stack};

let config = DrizzleConfig::x2()  // 2x output resolution
    .with_pixfrac(0.8)            // pixel fraction
    .with_kernel(DrizzleKernel::Square);

let result = drizzle_stack(&paths, &transforms, None, &config, progress)?;
// result.image is 2x the input resolution
// result.coverage shows data coverage per pixel
```

### Gradient Removal

```rust
use lumos::stacking::{GradientRemovalConfig, remove_gradient};

// Polynomial gradient removal (degree 1-4)
let config = GradientRemovalConfig::polynomial(2); // quadratic
let result = remove_gradient(&pixels, width, height, &config)?;

// RBF for complex gradients
let config = GradientRemovalConfig::rbf(0.5);
let result = remove_gradient(&pixels, width, height, &config)?;
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

### Drizzle Parameters

| Parameter | Recommendation | Notes |
|-----------|----------------|-------|
| Scale 1.5x | Conservative | Good for 3-point dithers |
| Scale 2.0x | Standard | Requires good dithering (default) |
| Scale 3.0x+ | Aggressive | Requires excellent dithering |
| pixfrac 0.7-0.8 | Recommended | Balance of resolution and noise |
| pixfrac 0.4-0.6 | Aggressive | Needs excellent data |

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

## Gradient Removal

Post-stack gradient removal for sky gradients caused by light pollution, moon glow, or vignetting.

### Algorithm

1. **Sample Placement**: Generate grid avoiding stars (uses brightness threshold)
2. **Model Fitting**:
   - Polynomial (degree 1-4): Fast, good for simple gradients
   - RBF (thin-plate spline): Better for complex, non-uniform gradients
3. **Correction**:
   - Subtract: For additive gradients (light pollution)
   - Divide: For multiplicative effects (vignetting)

### When to Use Each Method

| Method | Best For |
|--------|----------|
| Polynomial(1) | Simple linear gradients |
| Polynomial(2) | Parabolic gradients (default) |
| Polynomial(3-4) | Complex gradients (risk of overcorrection) |
| RBF | Non-uniform, rotating gradients |
| Subtract | Light pollution, moon glow |
| Divide | Vignetting |

---

## Future Improvements

### High Priority

1. **Adaptive Rejection Selection**: Auto-select method based on frame count
2. **Asymmetric Sigma Clipping**: Separate thresholds for high/low outliers
3. **SIMD-Optimized Rejection**: Vectorize inner loops for 2-4x speedup

### Medium Priority

4. **CFA-Aware Drizzle**: Raw sensor data support
5. **Robust Scale Estimators**: Sn/Qn estimators for non-Gaussian data
6. **Stacking Presets**: Pre-configured settings for common scenarios

### Advanced

7. **Per-Pixel Noise Weighting**: Weight = 1/variance per pixel
8. **GPU Acceleration**: Compute shader for sigma clipping
9. **Live Stacking**: Real-time preview during capture
10. **Multi-Session Integration**: Cross-session normalization and weighting
