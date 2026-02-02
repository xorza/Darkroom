# Star Detection Module

This document explains how to use the star detection algorithm and describes the internal workings of each submodule.

## Overview

The star detection module (`lumos::star_detection`) identifies stellar sources in astronomical images through a multi-stage pipeline that handles noise reduction, background estimation, source detection, sub-pixel centroiding, and quality filtering.

## Quick Start

### Basic Usage

```rust
use lumos::star_detection::{detect_stars, DetectionConfig, DetectedStar};

// Load your image data as a grayscale f32 slice
let image: &[f32] = &image_data;
let width = 1920;
let height = 1080;

// Use default configuration
let config = DetectionConfig::default();

// Detect stars
let stars: Vec<DetectedStar> = detect_stars(image, width, height, &config);

// Each star contains:
// - x, y: sub-pixel centroid position
// - intensity: peak brightness
// - background: local background level
// - snr: signal-to-noise ratio
// - fwhm: full width at half maximum (if computed)
// - sharpness, roundness: morphological metrics
```

### Custom Configuration

```rust
use lumos::star_detection::{DetectionConfig, BackgroundConfig, CentroidConfig};

let config = DetectionConfig {
    // Detection threshold in sigma above background
    sigma_threshold: 5.0,
    
    // Minimum/maximum star size in pixels
    min_star_radius: 2.0,
    max_star_radius: 25.0,
    
    // Background estimation settings
    background: BackgroundConfig {
        mesh_size: 64,        // Grid cell size for background mesh
        filter_size: 3,       // Median filter size for mesh smoothing
        sigma_clip: 3.0,      // Sigma clipping for outlier rejection
        iterations: 3,        // Number of sigma-clipping iterations
    },
    
    // Centroiding settings
    centroid: CentroidConfig {
        method: CentroidMethod::Gaussian,  // or Barycenter, Quadratic
        box_size: 7,                       // Fitting region size
        max_iterations: 10,                // For iterative methods
        tolerance: 0.01,                   // Convergence threshold
    },
    
    // Quality filters
    min_snr: 5.0,              // Minimum signal-to-noise ratio
    min_sharpness: 0.3,        // Filter extended sources
    max_sharpness: 0.9,        // Filter cosmic rays / hot pixels
    min_roundness: -0.5,       // Filter elongated sources
    max_roundness: 0.5,
    
    // Enable cosmic ray rejection
    reject_cosmic_rays: true,
    
    // Saturation level (stars above this are flagged)
    saturation_level: 65000.0,
};
```

## Pipeline Stages

The detection pipeline consists of five main stages:

```
Input Image
    │
    ▼
┌──────────────────────┐
│  1. Preprocessing    │  ── Cosmic ray rejection, median filtering
└──────────────────────┘
    │
    ▼
┌──────────────────────┐
│  2. Background       │  ── Mesh-based background/noise estimation
│     Estimation       │
└──────────────────────┘
    │
    ▼
┌──────────────────────┐
│  3. Detection        │  ── Convolution + thresholding + peak finding
└──────────────────────┘
    │
    ▼
┌──────────────────────┐
│  4. Centroiding      │  ── Sub-pixel position refinement
└──────────────────────┘
    │
    ▼
┌──────────────────────┐
│  5. Filtering        │  ── Quality metrics, deblending, validation
└──────────────────────┘
    │
    ▼
Detected Stars
```

## Module Architecture

### `background/` - Background Estimation

Estimates the sky background and noise level across the image using a mesh-based approach.

**Algorithm:**
1. Divide image into a grid of cells (mesh)
2. For each cell, compute statistics (mean, median, sigma) with sigma-clipping
3. Apply median filter to smooth the background mesh
4. Interpolate (bilinear) to full image resolution

**Key Functions:**
- `estimate_background()` - Main entry point
- `compute_mesh_statistics()` - Per-cell statistics with sigma clipping
- `interpolate_background()` - Bilinear mesh interpolation

**Why mesh-based?** Direct per-pixel estimation is too slow. The mesh approach assumes background varies slowly across the image, which is valid for most astronomical images.

### `detection/` - Source Detection

Finds candidate star positions using matched-filter convolution and peak detection.

**Algorithm:**
1. **Convolution**: Convolve background-subtracted image with a Gaussian kernel matching typical star PSF
2. **Thresholding**: Identify pixels exceeding `sigma_threshold × local_noise`
3. **Peak Finding**: Find local maxima using 8-connected neighborhood
4. **Segmentation**: Group connected pixels into source regions

**Key Functions:**
- `detect_sources()` - Main detection pipeline
- `convolve_gaussian()` - Matched filter convolution
- `find_local_maxima()` - Peak detection with non-maximum suppression
- `segment_sources()` - Connected component labeling

**SIMD Optimization:** The convolution and peak detection stages use SIMD (AVX2/SSE on x86_64, NEON on ARM) for significant speedup on large images.

### `centroid/` - Sub-pixel Centroiding

Refines star positions to sub-pixel accuracy using various fitting methods.

**Available Methods:**

| Method | Accuracy | Speed | Best For |
|--------|----------|-------|----------|
| `Barycenter` | ~0.1 px | Fast | Quick estimates |
| `Quadratic` | ~0.05 px | Fast | Well-sampled PSFs |
| `Gaussian` | ~0.02 px | Medium | General use |
| `Moffat` | ~0.01 px | Slow | Undersampled PSFs |

**Gaussian Fitting Algorithm:**
1. Extract star subimage (box_size × box_size)
2. Fit 2D Gaussian: `I(x,y) = A × exp(-((x-x₀)²/2σₓ² + (y-y₀)²/2σᵧ²)) + B`
3. Iterate until convergence (Levenberg-Marquardt)
4. Return fitted centroid (x₀, y₀) and FWHM from σ

**Key Functions:**
- `compute_centroid()` - Dispatch to selected method
- `barycenter_centroid()` - Intensity-weighted center of mass
- `quadratic_centroid()` - Parabolic interpolation of peak
- `gaussian_centroid()` - 2D Gaussian fitting
- `compute_fwhm()` - Full width at half maximum estimation

### `convolution/` - Image Convolution

Provides efficient convolution operations for the detection stage.

**Features:**
- Separable kernel optimization (2D = 1D horizontal × 1D vertical)
- SIMD-accelerated inner loops
- Edge handling (mirror, zero, extend)

**Key Functions:**
- `convolve_2d()` - General 2D convolution
- `convolve_separable()` - Optimized for separable kernels
- `gaussian_kernel()` - Generate Gaussian kernels

### `cosmic_ray/` - Cosmic Ray Rejection

Identifies and masks cosmic rays and hot pixels using the Laplacian edge detection method.

**Algorithm (Laplacian/LA Cosmic):**
1. Compute Laplacian (second derivative) of image
2. Cosmic rays have sharp, high-contrast edges → large Laplacian values
3. Compare Laplacian to local noise to find outliers
4. Apply fine-structure test to avoid flagging star cores
5. Grow mask to include wings of cosmic ray events

**Key Functions:**
- `detect_cosmic_rays()` - Main detection routine
- `compute_laplacian()` - Edge detection kernel
- `fine_structure_test()` - Distinguish CRs from stars

**SIMD Optimization:** Laplacian computation uses SIMD for 2-3x speedup.

### `deblend/` - Source Deblending

Separates overlapping stars that were merged during initial detection.

**Algorithm:**
1. For each source region, find multiple local maxima
2. If multiple peaks exist, use watershed segmentation to split
3. Assign pixels to nearest peak based on intensity gradient
4. Create separate source entries for each deblended component

**Key Functions:**
- `deblend_sources()` - Main deblending routine
- `find_saddle_points()` - Locate valleys between peaks
- `watershed_segment()` - Split merged sources

### `median_filter/` - Median Filtering

Provides robust noise reduction for preprocessing.

**Features:**
- Variable kernel sizes (3×3, 5×5, 7×7)
- Optimized histogram-based algorithm for larger kernels
- Used in background mesh smoothing and cosmic ray rejection

**Key Functions:**
- `median_filter_2d()` - 2D median filter
- `running_median()` - 1D running median (O(log n) per pixel)

## Data Structures

### `DetectedStar`

```rust
#[derive(Debug, Clone)]
pub struct DetectedStar {
    /// Sub-pixel X position (0-indexed from left)
    pub x: f64,
    /// Sub-pixel Y position (0-indexed from top)
    pub y: f64,
    /// Peak intensity (background-subtracted)
    pub intensity: f32,
    /// Local background level
    pub background: f32,
    /// Signal-to-noise ratio
    pub snr: f32,
    /// Full width at half maximum (pixels)
    pub fwhm: f32,
    /// Sharpness metric (0-1, higher = more point-like)
    pub sharpness: f32,
    /// Roundness metric (-1 to 1, 0 = circular)
    pub roundness: f32,
    /// True if star appears saturated
    pub saturated: bool,
}
```

### `DetectionConfig`

Main configuration structure controlling all pipeline parameters. See the "Custom Configuration" example above.

## Quality Metrics

### Signal-to-Noise Ratio (SNR)

```
SNR = (peak_intensity - background) / noise
```

Where `noise` is the local RMS from background estimation. Stars with SNR below `min_snr` are rejected.

### Sharpness

Measures how point-like a source is compared to the PSF:

```
sharpness = (peak - neighbors_mean) / peak
```

- Low sharpness → extended source (galaxy, nebula)
- High sharpness → cosmic ray or hot pixel
- Typical stars: 0.3 - 0.8

### Roundness

Measures ellipticity:

```
roundness = (σx - σy) / (σx + σy)
```

- 0 = perfectly circular
- Positive = elongated horizontally
- Negative = elongated vertically
- Typical stars: -0.3 to 0.3

## Performance Considerations

### SIMD Optimization

The following operations are SIMD-accelerated:
- Gaussian convolution (detection stage)
- Laplacian computation (cosmic ray rejection)
- Peak finding (detection stage)

Typical speedup: 2-4x on modern CPUs.

### Memory Usage

Background estimation allocates:
- Background mesh: `(width/mesh_size) × (height/mesh_size) × 4 bytes`
- Full background map: `width × height × 4 bytes`

For a 20 megapixel image with mesh_size=64:
- Mesh: ~20 KB
- Full map: ~80 MB

### Recommended Settings by Use Case

**Fast preview (live stacking):**
```rust
DetectionConfig {
    sigma_threshold: 8.0,
    centroid: CentroidConfig {
        method: CentroidMethod::Quadratic,
        ..Default::default()
    },
    reject_cosmic_rays: false,
    ..Default::default()
}
```

**High accuracy (astrometry/photometry):**
```rust
DetectionConfig {
    sigma_threshold: 3.0,
    centroid: CentroidConfig {
        method: CentroidMethod::Gaussian,
        max_iterations: 20,
        tolerance: 0.001,
        ..Default::default()
    },
    min_snr: 10.0,
    reject_cosmic_rays: true,
    ..Default::default()
}
```

**Crowded fields:**
```rust
DetectionConfig {
    background: BackgroundConfig {
        mesh_size: 32,  // Finer mesh for variable background
        ..Default::default()
    },
    min_star_radius: 1.5,
    // Enable deblending (on by default)
    ..Default::default()
}
```

## Troubleshooting

### Too few stars detected

1. **Lower `sigma_threshold`** (try 3.0-4.0)
2. **Check background estimation** - mesh_size may be too large for variable backgrounds
3. **Verify image calibration** - uncalibrated images have higher noise

### Too many false detections

1. **Raise `sigma_threshold`** (try 6.0-8.0)
2. **Enable cosmic ray rejection** if not already on
3. **Tighten sharpness/roundness filters**
4. **Check for hot pixels** - may need dark frame subtraction

### Poor centroid accuracy

1. **Use Gaussian or Moffat centroiding** instead of Barycenter
2. **Increase `box_size`** for undersampled images
3. **Ensure stars aren't saturated** - saturated cores break centroiding

### Slow performance

1. **Increase `mesh_size`** for background estimation
2. **Use simpler centroid method** (Quadratic or Barycenter)
3. **Disable cosmic ray rejection** for pre-calibrated images
4. **Reduce image size** if possible

## Integration with Registration

The star detection output feeds directly into the registration pipeline:

```rust
use lumos::star_detection::{detect_stars, DetectionConfig};
use lumos::registration::{register_images, RegistrationConfig};

// Detect stars in reference image
let ref_stars = detect_stars(&ref_image, width, height, &det_config);

// Detect stars in target image  
let target_stars = detect_stars(&target_image, width, height, &det_config);

// Convert to point format for registration
let ref_points: Vec<(f64, f64)> = ref_stars.iter().map(|s| (s.x, s.y)).collect();
let target_points: Vec<(f64, f64)> = target_stars.iter().map(|s| (s.x, s.y)).collect();

// Register images using detected stars
let transform = register_images(&ref_points, &target_points, &reg_config);
```

See [REGISTRATION.md](../registration/REGISTRATION.md) for details on image registration.
