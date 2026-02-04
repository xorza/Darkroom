# Star Detection Module

This document explains how to use the star detection algorithm and describes the internal workings of each submodule.

## Overview

The star detection module (`lumos::star_detection`) identifies stellar sources in astronomical images through a multi-stage pipeline that handles noise reduction, background estimation, matched filtering, source detection, deblending, sub-pixel centroiding, and quality filtering.

## Quick Start

### Basic Usage

```rust
use lumos::star_detection::{StarDetector, Config, Star};

// Create detector with default configuration
let mut detector = StarDetector::new();

// Detect stars in an AstroImage
let result = detector.detect(&image);

// Each star contains:
// - pos: DVec2 sub-pixel centroid position
// - flux: total background-subtracted flux
// - fwhm: full width at half maximum (pixels)
// - eccentricity: shape metric (0=circular, 1=elongated)
// - snr: signal-to-noise ratio
// - peak: peak pixel value
// - sharpness, roundness1, roundness2: morphological metrics
// - laplacian_snr: L.A.Cosmic cosmic ray metric
for star in &result.stars {
    println!("Star at ({:.1}, {:.1}), SNR={:.1}, FWHM={:.2}",
        star.pos.x, star.pos.y, star.snr, star.fwhm);
}

// Reuse detector for batch processing (buffer pool reuse)
for image in images {
    let result = detector.detect(&image);
}
```

### Custom Configuration

```rust
use lumos::star_detection::{
    Config, StarDetector, BackgroundRefinement,
    CentroidMethod, NoiseModel,
};

let config = Config {
    // Background estimation
    sigma_threshold: 4.0,           // Detection sigma above background
    tile_size: 64,                  // Grid cell size for background mesh
    sigma_clip_iterations: 5,       // Sigma-clipping iterations per tile
    background_mask_dilation: 3,    // Object mask dilation radius
    min_unmasked_fraction: 0.3,     // Min unmasked pixels per tile
    refinement: BackgroundRefinement::None, // or Iterative/AdaptiveSigma

    // PSF and matched filtering
    expected_fwhm: 4.0,             // Expected FWHM for matched filter (0 = disable)
    psf_axis_ratio: 1.0,            // PSF ellipticity (1.0 = circular)
    psf_angle: 0.0,                 // PSF rotation angle (radians)
    auto_estimate_fwhm: false,      // Auto-estimate FWHM from bright stars

    // Detection parameters
    min_area: 5,                    // Minimum connected pixels
    max_area: 500,                  // Maximum connected pixels
    edge_margin: 10,                // Border exclusion zone

    // Quality filtering
    min_snr: 10.0,                  // Minimum signal-to-noise ratio
    max_eccentricity: 0.6,          // Filter elongated sources
    max_sharpness: 0.7,             // Filter cosmic rays / hot pixels
    max_roundness: 0.5,             // Filter asymmetric sources
    max_fwhm_deviation: 3.0,        // FWHM outlier rejection (MAD units)
    duplicate_min_separation: 8.0,  // Deduplication distance

    // Centroiding
    centroid_method: CentroidMethod::WeightedMoments, // or GaussianFit, MoffatFit

    // Deblending
    deblend_min_separation: 3,      // Min peak separation for deblending
    deblend_min_prominence: 0.3,    // Min peak prominence fraction
    deblend_n_thresholds: 0,        // 0=local maxima, 32+=multi-threshold
    deblend_min_contrast: 0.005,    // Min contrast for multi-threshold

    // Optional noise model for accurate SNR
    noise_model: Some(NoiseModel::new(1.5, 5.0)), // gain=1.5 e-/ADU, read_noise=5 e-
    defect_map: None,

    ..Default::default()
};

let mut detector = StarDetector::from_config(config);
let result = detector.detect(&image);
```

### Preset Configurations

```rust
// Wide-field imaging (larger stars, relaxed filtering)
let config = Config::wide_field();

// High-resolution imaging (smaller stars, stricter filtering)
let config = Config::high_resolution();

// Crowded fields (aggressive deblending, iterative background)
let config = Config::crowded_field();

// Nebulous fields (adaptive sigma thresholding)
let config = Config::nebulous_field();

// Precise ground-based (Moffat fitting)
let config = Config::precise_ground();
```

## Pipeline Stages

The detection pipeline consists of 6 stages, orchestrated by `StarDetector::detect()`:

```
Input AstroImage
    |
    v
+------------------------+
|  1. Prepare            |  -- Grayscale conversion, defect masking,
|                        |     3x3 median filter (CFA sensors)
+------------------------+
    |
    v
+------------------------+
|  2. Background         |  -- Tile-based background/noise estimation
|                        |     with optional iterative refinement
+------------------------+
    |
    v
+------------------------+
|  3. FWHM Estimation    |  -- Auto-estimate FWHM from bright stars (optional)
+------------------------+
    |
    v
+------------------------+
|  4. Detect             |  -- Matched filter (optional), threshold mask,
|                        |     connected component labeling, deblending,
|                        |     region extraction and filtering
+------------------------+
    |
    v
+------------------------+
|  5. Measure            |  -- Sub-pixel centroiding + quality metrics
+------------------------+
    |
    v
+------------------------+
|  6. Filter             |  -- SNR, eccentricity, sharpness, roundness,
|                        |     FWHM outliers, deduplication, sort by flux
+------------------------+
    |
    v
DetectionResult { stars, diagnostics }
```

## Module Architecture

### `detector/` - Pipeline Orchestrator

Contains `StarDetector`, the main entry point that coordinates all pipeline stages.

**Key Types:**
- `StarDetector` - Main detector with buffer pool for efficient batch processing
- `DetectionResult` - Final output with stars and diagnostics
- `Diagnostics` - Per-stage statistics for debugging and tuning

**Submodule `detector/stages/`:**
- `prepare.rs` - Image preparation (grayscale, defects, CFA filter)
- `background.rs` - Re-exports background estimation functions
- `fwhm.rs` - FWHM auto-estimation from bright stars
- `detect.rs` - Threshold mask, labeling, deblending, region extraction
- `measure.rs` - Sub-pixel centroiding wrapper
- `filter.rs` - Quality filtering, FWHM outliers, duplicate removal

### `background/` - Background Estimation

Estimates the sky background and noise level using a tile-based approach with sigma-clipped statistics.

**Algorithm:**
1. Divide image into tiles (configurable size, default 64px)
2. For each tile, compute sigma-clipped median and MAD (using up to 1024 samples)
3. Apply 3x3 median filter to the tile grid (rejects outlier tiles)
4. Bilinearly interpolate from tile centers to full image resolution

**Refinement strategies (mutually exclusive):**
- `Iterative`: Mask detected sources, re-estimate background excluding them. Best for crowded fields.
- `AdaptiveSigma`: Per-pixel detection thresholds based on local contrast. Higher thresholds in nebulous regions reduce false positives.

**Key Types:**
- `BackgroundEstimate` - Holds per-pixel background, noise, and optional adaptive sigma buffers
- `TileGrid` - Grid of per-tile statistics (median, sigma, adaptive_sigma)

### `convolution/` - Matched Filter Convolution

Convolves the background-subtracted image with a Gaussian kernel matched to the expected PSF.

**Features:**
- Separable convolution for circular PSFs: O(n*k) per pixel
- Full 2D convolution for elliptical PSFs: O(n*k^2)
- Kernel radius = 3*sigma (captures 99.7% of Gaussian energy)
- Mirror boundary handling
- SIMD-accelerated: AVX2+FMA, SSE4.1, NEON

**Key Functions:**
- `matched_filter()` - Background subtraction + convolution
- `gaussian_convolve()` - Separable Gaussian convolution
- `elliptical_gaussian_convolve()` - Full 2D elliptical convolution
- `gaussian_kernel_1d()` - Kernel generation (normalized to sum=1)

### `threshold_mask/` - Threshold Mask Creation

Creates a bit-packed binary mask where pixels exceed the detection threshold.

**Features:**
- Bit-packed storage: 1 bit per pixel in u64 words (8x memory savings)
- Three variants: standard, pre-filtered, adaptive sigma
- SIMD-accelerated: SSE4.1, NEON

### `mask_dilation/` - Morphological Dilation

Separable morphological dilation to connect fragmented detections.

**Features:**
- Horizontal + vertical separable passes
- Word-level bit smearing for fast horizontal dilation
- Parallel row/column processing

### `labeling/` - Connected Component Labeling

Labels connected components in the threshold mask using efficient union-find.

**Algorithm:**
- RLE-based scanning with word-level bit operations
- Lock-free union-find with atomic operations for parallel merging
- Block-based parallel labeling with boundary merging

**Key Types:**
- `LabelMap` - Connected component labels (Buffer2<u32>)

### `deblend/` - Source Deblending

Separates overlapping stars that were merged during detection.

**Two algorithms:**

| Algorithm | Speed | Accuracy | Best For |
|-----------|-------|----------|----------|
| Local Maxima | Fast | Good for well-separated | Default (`n_thresholds=0`) |
| Multi-Threshold | Slower | Better for crowded fields | `n_thresholds=32` |

**Local Maxima Deblending:**
1. Find 8-connected local maxima in each component
2. Filter by prominence and separation
3. Assign pixels to nearest peak via Voronoi partitioning
4. Uses `ArrayVec<_, MAX_PEAKS=8>` (stack-allocated)

**Multi-Threshold Deblending (SExtractor-style):**
1. Apply N threshold levels between detection threshold and peak
2. Build tree of sub-components at each level
3. Split branches whose flux exceeds minimum contrast criterion
4. Grid-based pixel lookup for fast neighbor access

**Key Types:**
- `Region` - Detected region with bbox, peak position, peak value, area

### `centroid/` - Sub-pixel Centroiding

Refines star positions to sub-pixel accuracy and computes quality metrics.

**Available Methods:**

| Method | Accuracy | Speed | Best For |
|--------|----------|-------|----------|
| `WeightedMoments` | ~0.05 px | Fast | Default, general use |
| `GaussianFit` | ~0.01 px | ~8x slower | Well-sampled PSFs |
| `MoffatFit { beta }` | ~0.01 px | ~8x slower | Atmospheric seeing |

**WeightedMoments Algorithm:**
1. Extract stamp (radius = 1.75 * FWHM, clamped to [4, 15] pixels)
2. Iterative weighted centroid with Gaussian weighting kernel
3. Converge when delta < 0.001 pixels or 10 iterations max
4. NEON SIMD implementation for ARM

**GaussianFit Algorithm:**
1. Fit `f(x,y) = A * exp(-((x-x0)^2/2sig_x^2 + (y-y0)^2/2sig_y^2)) + B`
2. 6 parameters: position, amplitude, sigma_x, sigma_y, background
3. Levenberg-Marquardt optimization with analytical Jacobian
4. SIMD-accelerated chi^2 and Jacobian: AVX2+FMA, SSE4.1, NEON
5. Optional inverse-variance weighting with CCD noise model

**MoffatFit Algorithm:**
1. Fit `f(x,y) = A * (1 + ((x-x0)^2+(y-y0)^2)/alpha^2)^(-beta) + B`
2. Beta controls wing falloff: 2.5 typical for ground-based, 4.5 for space
3. Same L-M optimizer infrastructure as Gaussian

**Quality Metrics (computed after centroiding):**
- Flux: sum of background-subtracted pixels in stamp
- FWHM: from second moments (sigma -> FWHM conversion)
- Eccentricity: from covariance matrix eigenvalues
- SNR: full CCD noise model when gain/read_noise available
- Sharpness: peak / core_flux (cosmic ray discriminator)
- Roundness1 (GROUND): (Hx - Hy) / (Hx + Hy) from marginal distributions
- Roundness2 (SROUND): bilateral symmetry metric
- Laplacian SNR: L.A.Cosmic metric per star

### `cosmic_ray/` - Cosmic Ray Detection

L.A.Cosmic-based cosmic ray identification (van Dokkum 2001, PASP 113, 1420).

**Core principle:** Cosmic rays have sharper edges than astronomical sources (which are smoothed by the PSF). The Laplacian responds strongly to these sharp edges.

**Used in two ways:**
1. **Per-star metric** (`compute_laplacian_snr()`): Computes Laplacian SNR at each star's position during centroiding. Stars with high values (>50) are flagged as cosmic rays. This is the primary CR rejection method in the pipeline.
2. **Full-frame detection** (`detect_cosmic_rays()`): Standalone function for image-level CR detection and masking.

**Key Functions:**
- `compute_laplacian()` - 3x3 discrete Laplacian with SIMD acceleration
- `compute_laplacian_snr()` - Per-star Laplacian SNR metric
- `compute_fine_structure()` - Small-scale structure (original - median3)
- `detect_cosmic_rays()` - Full-frame L.A.Cosmic detection

### `median_filter/` - Median Filtering

3x3 median filter for removing Bayer pattern artifacts from CFA sensors.

**Features:**
- Sorting-network-based median (optimal compare-swap sequences)
- SIMD-accelerated: SSE4.1/AVX2, NEON
- Parallel row processing via rayon

### `buffer_pool.rs` - Buffer Pool

Pools `Buffer2<f32>`, `BitBuffer2`, and `Buffer2<u32>` for reuse across multiple `detect()` calls. Avoids repeated allocation when processing image sequences.

### `defect_map.rs` - Sensor Defect Masking

Masks hot pixels, dead pixels, and bad columns/rows by replacing them with the local median of non-defective neighbors. Applied before detection.

## Data Structures

### `Star`

```rust
#[derive(Debug, Clone, Copy)]
pub struct Star {
    /// Sub-pixel position (DVec2).
    pub pos: DVec2,
    /// Total background-subtracted flux.
    pub flux: f32,
    /// Full width at half maximum (pixels).
    pub fwhm: f32,
    /// Eccentricity (0=circular, 1=elongated).
    pub eccentricity: f32,
    /// Signal-to-noise ratio.
    pub snr: f32,
    /// Peak pixel value (for saturation detection).
    pub peak: f32,
    /// Sharpness (peak / flux_in_core). High = cosmic ray.
    pub sharpness: f32,
    /// DAOFIND GROUND roundness from marginal distributions.
    pub roundness1: f32,
    /// DAOFIND SROUND symmetry-based roundness.
    pub roundness2: f32,
    /// L.A.Cosmic Laplacian SNR metric.
    pub laplacian_snr: f32,
}
```

### `Config`

Flat configuration struct with all parameters grouped by function:

```rust
pub struct Config {
    // Background estimation
    pub sigma_threshold: f32,
    pub tile_size: usize,
    pub sigma_clip_iterations: usize,
    pub background_mask_dilation: usize,
    pub min_unmasked_fraction: f32,
    pub refinement: BackgroundRefinement,

    // PSF / matched filter
    pub expected_fwhm: f32,
    pub psf_axis_ratio: f32,
    pub psf_angle: f32,
    pub auto_estimate_fwhm: bool,
    // ... FWHM estimation parameters

    // Detection
    pub connectivity: Connectivity,
    pub min_area: usize,
    pub max_area: usize,
    pub edge_margin: usize,

    // Deblending
    pub deblend_min_separation: usize,
    pub deblend_min_prominence: f32,
    pub deblend_n_thresholds: usize,
    pub deblend_min_contrast: f32,

    // Centroiding
    pub centroid_method: CentroidMethod,
    pub local_background: LocalBackgroundMethod,
    // ... centroid parameters

    // Quality filtering
    pub min_snr: f32,
    pub max_eccentricity: f32,
    pub max_sharpness: f32,
    pub max_roundness: f32,
    pub max_fwhm_deviation: f32,
    pub duplicate_min_separation: f32,

    // Optional
    pub noise_model: Option<NoiseModel>,
    pub defect_map: Option<DefectMap>,
}
```

## Quality Metrics

### Signal-to-Noise Ratio (SNR)

With noise model (gain, read_noise):
```
SNR = flux / sqrt(flux/gain + npix * (sigma_sky^2 + read_noise^2/gain^2))
```

Without noise model (background-dominated):
```
SNR = flux / (sigma_sky * sqrt(npix))
```

### Sharpness

```
sharpness = peak_value / core_flux  (3x3 core region)
```

- Low sharpness (0.2-0.5): real stars (flux spread across PSF)
- High sharpness (>0.7): cosmic ray or hot pixel (flux concentrated)

### Roundness (DAOFIND-style)

**GROUND (roundness1):**
```
roundness1 = (Hx - Hy) / (Hx + Hy)
```
Where Hx, Hy are peak heights of marginal x/y distributions.

**SROUND (roundness2):**
RMS bilateral asymmetry of marginal distributions.
- 0 = perfectly symmetric
- Non-zero = asymmetric

### Eccentricity

From covariance matrix eigenvalues:
```
eccentricity = sqrt(1 - lambda2/lambda1)
```
- 0 = circular
- >0.6 = noticeably elongated

## Performance Considerations

### SIMD Optimization

SIMD-accelerated operations (2-4x speedup on modern CPUs):
- Background interpolation
- Gaussian convolution (row, column, 2D)
- Threshold mask creation
- 3x3 median filter
- Laplacian computation
- Gaussian/Moffat profile fitting (chi^2 + Jacobian)
- Centroid refinement (NEON)

Dispatch priority: AVX2+FMA -> SSE4.1 -> NEON -> scalar.

### Memory Usage

Background estimation allocates:
- Tile grid: `(width/tile_size) * (height/tile_size) * 12 bytes`
- Full background map: `width * height * 4 bytes`
- Full noise map: `width * height * 4 bytes`

Buffer pool reuses allocations across multiple `detect()` calls.

### Recommended Settings by Use Case

**Fast preview (live stacking):**
```rust
Config {
    sigma_threshold: 8.0,
    expected_fwhm: 0.0,  // Disable matched filter
    ..Default::default()
}
```

**High accuracy (astrometry/photometry):**
```rust
Config {
    sigma_threshold: 3.0,
    refinement: BackgroundRefinement::Iterative { iterations: 2 },
    centroid_method: CentroidMethod::GaussianFit,
    min_snr: 15.0,
    noise_model: Some(NoiseModel::new(1.5, 5.0)),
    ..Default::default()
}
```

**Crowded fields:**
```rust
Config::crowded_field()
// Or manually:
Config {
    tile_size: 32,  // Finer mesh for variable background
    refinement: BackgroundRefinement::Iterative { iterations: 2 },
    deblend_n_thresholds: 32,  // SExtractor-style multi-threshold
    deblend_min_separation: 2,
    ..Default::default()
}
```

**Nebulous backgrounds:**
```rust
Config::nebulous_field()
// Uses AdaptiveSigma refinement to raise thresholds in high-contrast regions
```

## Troubleshooting

### Too few stars detected

1. **Lower `sigma_threshold`** (try 3.0-4.0)
2. **Check `expected_fwhm`** - if too different from actual seeing, matched filter hurts
3. **Set `expected_fwhm: 0.0`** to disable matched filtering and detect without convolution
4. **Enable `auto_estimate_fwhm: true`** to let the pipeline estimate FWHM automatically
5. **Lower `min_area`** (try 3)
6. **Check background estimation** - tile_size may be too large for variable backgrounds

### Too many false detections

1. **Raise `sigma_threshold`** (try 6.0-8.0)
2. **Lower `max_sharpness`** to reject more cosmic rays
3. **Use `BackgroundRefinement::AdaptiveSigma`** for nebulous regions
4. **Use `BackgroundRefinement::Iterative`** for crowded fields
5. **Check for hot pixels** - use `DefectMap` or dark frame subtraction

### Poor centroid accuracy

1. **Use `CentroidMethod::GaussianFit`** instead of WeightedMoments
2. **Use `CentroidMethod::MoffatFit { beta: 2.5 }`** for atmospheric seeing
3. **Ensure `expected_fwhm` is close to actual** - affects stamp size
4. **Ensure stars aren't saturated** - saturated cores break centroiding

### Slow performance

1. **Increase `tile_size`** for background estimation
2. **Use `CentroidMethod::WeightedMoments`** (default, fastest)
3. **Set `expected_fwhm: 0.0`** to skip matched filter convolution
4. **Reuse `StarDetector`** across frames for buffer pool reuse

## Integration with Registration

The star detection output feeds directly into the registration pipeline:

```rust
use lumos::star_detection::{StarDetector, Config};

let mut detector = StarDetector::new();

// Detect stars in reference and target images
let ref_result = detector.detect(&ref_image);
let target_result = detector.detect(&target_image);

// Convert to point format for registration
let ref_points: Vec<(f64, f64)> = ref_result.stars.iter()
    .map(|s| (s.pos.x, s.pos.y))
    .collect();
let target_points: Vec<(f64, f64)> = target_result.stars.iter()
    .map(|s| (s.pos.x, s.pos.y))
    .collect();
```

## Algorithm References

- **Background estimation**: SExtractor (Bertin & Arnouts 1996, A&AS 117, 393)
- **Matched filtering**: DAOFIND (Stetson 1987, PASP 99, 191)
- **Cosmic ray rejection**: L.A.Cosmic (van Dokkum 2001, PASP 113, 1420)
- **Multi-threshold deblending**: SExtractor DEBLEND algorithm
- **Roundness metrics**: DAOFIND GROUND and SROUND (Stetson 1987)
