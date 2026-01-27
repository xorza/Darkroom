# Astrophotography Image Stacking Implementation Plan

## Overview

This document outlines a comprehensive plan for implementing state-of-the-art astrophotography image stacking with sub-pixel tracking. The implementation combines the best algorithms from leading software like PixInsight, Siril, and DeepSkyStacker, while leveraging GPU acceleration for performance.

**Last Updated**: January 2026

---

## Implementation Status Summary

| Phase | Status | Completion |
|-------|--------|------------|
| Phase 1: Core Foundation | **COMPLETE** | 100% |
| Phase 2: Sub-Pixel Registration | **COMPLETE** | 100% |
| Phase 3: Advanced Stacking | **PARTIAL** | 60% |
| Phase 4: Super-Resolution (Drizzle) | **NOT STARTED** | 0% |
| Phase 5: GPU Acceleration | **PARTIAL** | 30% |
| Phase 6: Polish & Advanced Features | **PARTIAL** | 20% |

---

## 1. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Astro Stacking Pipeline                             │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │  1. Input    │──▶│ 2. Calibrate │──▶│ 3. Register  │──▶│  4. Stack    │ │
│  │   Loader     │   │    Frames    │   │    Frames    │   │   & Output   │ │
│  └──────────────┘   └──────────────┘   └──────────────┘   └──────────────┘ │
│         │                  │                  │                  │         │
│         ▼                  ▼                  ▼                  ▼         │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │ FITS/RAW/    │   │ Bias/Dark/   │   │ Star Detect  │   │ Sigma Clip   │ │
│  │ TIFF support │   │ Flat Apply   │   │ Sub-pixel    │   │ Drizzle      │ │
│  │ Metadata     │   │ Hot/Cold px  │   │ Alignment    │   │ Integration  │ │
│  └──────────────┘   └──────────────┘   └──────────────┘   └──────────────┘ │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Core Modules

### 2.1 Image I/O Module [COMPLETE]

**Status**: Fully implemented

**Supported Formats**:
- FITS (Flexible Image Transport System) - primary format
- RAW camera formats (via rawloader and libraw)
- TIFF (16-bit, 32-bit float)
- PNG (8-bit, 16-bit)

**Implemented Features**:
- Parse FITS headers for exposure time, temperature, gain, date/time
- Support for monochrome, Bayer-pattern, and X-Trans CFA sensors
- Full demosaicing support
- Batch loading with parallel I/O

---

### 2.2 Calibration Module [COMPLETE]

**Status**: Fully implemented

#### 2.2.1 Calibration Frame Types

| Frame Type | Description | Status |
|------------|-------------|--------|
| **Bias** | Read noise pattern | Implemented |
| **Dark** | Thermal noise pattern | Implemented |
| **Flat** | Optical vignetting & dust | Implemented |
| **Flat-Dark** | Dark for flat exposure | Implemented |

#### 2.2.2 Calibration Pipeline

```rust
fn calibrate_light(light: &Image, masters: &CalibrationMasters) -> Image {
    // 1. Subtract master dark (removes thermal noise + bias)
    let corrected = light - masters.dark;
    
    // 2. Divide by normalized master flat (corrects vignetting)
    let flat_normalized = masters.flat / masters.flat.mean();
    let calibrated = corrected / flat_normalized;
    
    calibrated
}
```

#### 2.2.3 Master Frame Creation

Implemented with sigma-clipped mean and outlier rejection.

#### 2.2.4 Hot/Cold Pixel Detection & Correction [COMPLETE]

**Implemented Methods**:
- Statistical detection from dark frames (pixels > 5σ above median)
- Laplacian detection (LACosmic-style) for cosmic rays
- Correction via median of 8-connected neighbors

---

### 2.3 Star Detection Module [COMPLETE]

**Status**: Fully implemented with SIMD acceleration

#### 2.3.1 Detection Algorithm [COMPLETE]

**Implemented**:
- Background estimation with tile-based sigma-clipped median
- Initial detection with threshold: pixel > background + k×σ
- Connected component labeling
- Filtering by size, eccentricity, saturation
- Star deblending with multi-threshold approach
- SIMD acceleration (ARM NEON, x86 SSE4)

#### 2.3.2 Sub-Pixel Centroid Computation [COMPLETE]

**Implemented Methods**:
1. **Gaussian PSF Fitting** (~0.01-0.05 pixel accuracy)
2. **Moffat Profile Fitting** (~0.01 pixel accuracy)
3. **Iterative Weighted Centroid** (default, ~0.05 pixel accuracy)

#### 2.3.3 PSF Modeling [COMPLETE]

**Implemented**:
- Moffat profile fitting with configurable beta parameter
- Gaussian 2D fitting
- Quality metrics: FWHM, SNR, eccentricity, sharpness, roundness

---

### 2.4 Image Registration Module [COMPLETE]

**Status**: Fully implemented

#### 2.4.1 Coarse Registration: Triangle Matching [COMPLETE]

**Implemented**:
- Geometric hashing with scale/rotation invariant descriptors
- K-D tree spatial indexing for efficient neighbor queries
- Configurable star selection (brightest N with spatial distribution)
- RANSAC with Local Optimization (LO-RANSAC)

#### 2.4.2 Fine Registration: Sub-Pixel Methods [COMPLETE]

**Implemented**:
- Phase correlation with Hann windowing
- Sub-pixel refinement (Parabolic, Gaussian, Centroid methods)
- Optional iterative phase correlation for 0.01 pixel accuracy

#### 2.4.3 Transformation Models [COMPLETE]

| Model | DOF | Status |
|-------|-----|--------|
| Translation | 2 | Implemented |
| Euclidean | 3 | Implemented |
| Similarity | 4 | Implemented |
| Affine | 6 | Implemented |
| Homography | 8 | Implemented |

#### 2.4.4 Interpolation for Sub-Pixel Warping [COMPLETE]

**Implemented**:
- Lanczos-3 and Lanczos-4 (primary, best quality)
- Bicubic
- Bilinear
- Nearest neighbor
- SIMD-accelerated row processing
- GPU-accelerated warping via imaginarium

---

### 2.5 Stacking Module [PARTIAL]

**Status**: Core functionality complete, advanced rejection methods pending

#### 2.5.1 Normalization Methods [PARTIAL]

**Implemented**:
- Global normalization (scale and offset matching)

**Not Yet Implemented**:
- [ ] Local normalization (PixInsight-style tile-based)
- [ ] Per-session normalization for multi-night data

#### 2.5.2 Pixel Rejection Algorithms [PARTIAL]

| Method | Status | Notes |
|--------|--------|-------|
| Sigma Clipping | Implemented | Iterative with statistics tracking |
| Winsorized Sigma Clipping | **NOT IMPLEMENTED** | Replace outliers with boundary values |
| Linear Fit Clipping | **NOT IMPLEMENTED** | Best for sky gradients |
| Percentile Clipping | **NOT IMPLEMENTED** | Good for small stacks |
| GESD (Generalized ESD) | **NOT IMPLEMENTED** | Best for large datasets (>50 frames) |

#### 2.5.3 Integration Methods [PARTIAL]

| Method | Status |
|--------|--------|
| Mean | Implemented (SIMD-accelerated) |
| Median | Implemented (disk-based chunked processing) |
| Weighted Mean | **NOT IMPLEMENTED** |

#### 2.5.4 Drizzle Integration (Super-Resolution) [NOT IMPLEMENTED]

**Status**: Not yet implemented

**To Implement**:
- [ ] Output grid at higher resolution (1.5×, 2×, 3×)
- [ ] Droplet distribution with configurable pixfrac (0.5-0.9)
- [ ] Coverage map and normalization
- [ ] Multiple kernels (Square, Circular, Gaussian, Lanczos)
- [ ] Automatic dither detection

**Best Practices from HST Documentation**:
- pixfrac=0.8 optimal for four-point dithered data
- pixfrac should be slightly larger than output scale
- Target RMS/median < 0.2 on weight image
- Gaussian kernel recommended for point source photometry

---

### 2.6 GPU Acceleration [PARTIAL]

**Status**: Foundation in place, most components still CPU-only

#### 2.6.1 GPU-Accelerated Components

| Component | Status | Method |
|-----------|--------|--------|
| Image Warping | Implemented | wgpu compute shaders |
| FFT | **NOT IMPLEMENTED** | Currently using rustfft (CPU) |
| Sigma Clipping | **NOT IMPLEMENTED** | Parallel reduction needed |
| Star Detection | **NOT IMPLEMENTED** | Parallel threshold |
| Convolution | **NOT IMPLEMENTED** | Shared memory approach |

#### 2.6.2 Recommended Implementation Priority

1. **GPU FFT** - Highest impact for phase correlation
2. **GPU Sigma Clipping** - Major bottleneck for large stacks
3. **GPU Star Detection** - Benefits real-time preview
4. **Batch Pipeline** - Overlap compute/transfer

---

## 3. Advanced Features

### 3.1 Automatic Quality Weighting [NOT IMPLEMENTED]

**Status**: Not yet implemented

**To Implement (SubframeSelector-style)**:
- [ ] Per-frame quality metrics (FWHM, eccentricity, SNR, star count)
- [ ] Quality scoring function: `weight = (SNR^a × (1/FWHM)^b × (1/eccentricity)^c) / noise`
- [ ] Integration with weighted mean stacking
- [ ] Frame rejection based on quality threshold

### 3.2 Comet/Asteroid Stacking Mode [NOT IMPLEMENTED]

**Status**: Not yet implemented

**To Implement**:
- [ ] Dual-stack approach (stack on stars / stack on comet)
- [ ] User-provided comet positions at t₁ and t₂
- [ ] Frame-specific offset based on timestamp
- [ ] Composite output combining stars from one stack, comet from other

### 3.3 Multi-Session Integration [NOT IMPLEMENTED]

**Status**: Not yet implemented

**To Implement**:
- [ ] Per-session quality assessment
- [ ] Local normalization to match backgrounds across nights
- [ ] Session-weighted integration
- [ ] Gradient removal post-stack

### 3.4 Distortion Correction [PARTIAL]

**Implemented**:
- Thin-plate spline (TPS) for local distortion modeling
- Distortion map computation and visualization

**Not Yet Implemented**:
- [ ] Radial distortion models (barrel/pincushion): r' = r(1 + k₁r² + k₂r⁴)
- [ ] Tangential distortion correction
- [ ] Field curvature correction
- [ ] Astrometric solution via Gaia/UCAC4 catalog matching

---

## 4. Implementation Phases

### Phase 1: Core Foundation [COMPLETE]
- [x] Image I/O (FITS, TIFF, RAW)
- [x] Basic calibration (bias, dark, flat subtraction)
- [x] Hot pixel detection and removal
- [x] Star detection with centroid computation
- [x] Basic registration (translation only)
- [x] Simple mean/median stacking

### Phase 2: Sub-Pixel Registration [COMPLETE]
- [x] Triangle matching algorithm
- [x] RANSAC transformation estimation
- [x] Phase correlation with sub-pixel refinement
- [x] Lanczos interpolation
- [x] Affine/homography transformations

### Phase 3: Advanced Stacking [PARTIAL - 60%]
- [x] Sigma clipping (basic)
- [ ] Winsorized sigma clipping
- [ ] Linear fit clipping
- [ ] Percentile clipping
- [ ] Generalized ESD
- [ ] Weighted integration
- [ ] Local normalization
- [x] Quality metrics computation
- [ ] Automatic frame weighting

### Phase 4: Super-Resolution [NOT STARTED]
- [ ] Drizzle integration
- [ ] Multiple kernel support (Square, Gaussian, Lanczos)
- [ ] Automatic dither detection
- [ ] Coverage map handling

### Phase 5: GPU Acceleration [PARTIAL - 30%]
- [x] GPU image warping
- [ ] GPU FFT for phase correlation
- [ ] GPU sigma clipping
- [ ] GPU star detection
- [ ] Batch processing pipeline

### Phase 6: Polish & Advanced Features [PARTIAL - 20%]
- [ ] Comet stacking mode
- [x] Distortion correction (TPS only)
- [ ] Astrometric solution
- [ ] Multi-session support
- [ ] Real-time preview
- [ ] Deep learning denoising integration

---

## 5. Suggested Improvements Based on Best Practices

### 5.1 High Priority Additions

#### 5.1.1 Weighted Mean Integration
Modern best practice from PixInsight and Siril. Should weight frames by:
- SNR (signal-to-noise ratio)
- FWHM (seeing quality)
- Eccentricity (tracking quality)
- Background noise level

#### 5.1.2 Linear Fit Clipping
From Siril documentation: "Fits the best straight line of the pixel stack and rejects outliers. This algorithm performs very well with large stacks and images containing sky gradients with differing spatial distributions."

#### 5.1.3 GESD Rejection
From Siril: "Generalized Extreme Studentized Deviate Test shows excellent performances with large datasets of more than 50 images."

### 5.2 Medium Priority Additions

#### 5.2.1 Local Normalization
Essential for multi-session data. PixInsight-style approach:
- Divide image into tiles (128×128)
- Compute local median and scale
- Smooth interpolation between tiles

#### 5.2.2 Drizzle with Optimal Parameters
Based on HST best practices:
- Default pixfrac=0.8 for typical 4-point dither
- Gaussian kernel for point sources
- Coverage map quality check (RMS/median < 0.2)

### 5.3 Future Considerations

#### 5.3.1 Deep Learning Integration
Recent advances (2025) in self-supervised denoising:
- TDR method from Nature Astronomy paper
- Self2Self network for single-image denoising
- Could enhance weak structures by 10× (equivalent to 10× exposure time)

#### 5.3.2 AI-Assisted Tools
Consider integration points for:
- StarNet++ style star removal
- NoiseXTerminator-style denoising
- GraXpert-style gradient removal

---

## 6. Data Structures

```rust
/// Core image representation
#[derive(Debug)]
pub struct AstroImage {
    pub data: ndarray::Array2<f32>,
    pub width: u32,
    pub height: u32,
    pub metadata: ImageMetadata,
}

#[derive(Debug)]
pub struct ImageMetadata {
    pub exposure_time: f64,
    pub temperature: Option<f64>,
    pub gain: Option<u32>,
    pub timestamp: DateTime<Utc>,
    pub filter: Option<String>,
    pub bayer_pattern: Option<BayerPattern>,
}

/// Detected star with sub-pixel position
#[derive(Debug)]
pub struct Star {
    pub x: f64,
    pub y: f64,
    pub flux: f64,
    pub fwhm: f64,
    pub eccentricity: f64,
    pub snr: f64,
}

/// Transformation between frames
#[derive(Debug)]
pub enum Transform {
    Translation { dx: f64, dy: f64 },
    Euclidean { dx: f64, dy: f64, angle: f64 },
    Affine { matrix: [[f64; 3]; 2] },
    Homography { matrix: [[f64; 3]; 3] },
}

/// Stacking configuration
#[derive(Debug)]
pub struct StackConfig {
    pub rejection: RejectionMethod,
    pub integration: IntegrationMethod,
    pub normalization: NormalizationMethod,
    pub drizzle: Option<DrizzleConfig>,
}

#[derive(Debug)]
pub enum RejectionMethod {
    None,
    SigmaClip { kappa: f64, iterations: u32 },
    WinsorizedSigmaClip { kappa: f64, iterations: u32 },
    LinearFitClip { kappa_low: f64, kappa_high: f64 },
    Percentile { low: f64, high: f64 },
    GESD { alpha: f64, max_outliers: usize },
}

#[derive(Debug)]
pub enum IntegrationMethod {
    Mean,
    Median,
    WeightedMean { weights: Vec<f64> },
}

#[derive(Debug)]
pub struct DrizzleConfig {
    pub scale: f64,        // Output scale (1.5, 2.0, 3.0)
    pub drop_size: f64,    // Pixel fraction (0.5-0.9), default 0.8
    pub kernel: DrizzleKernel,
}

#[derive(Debug)]
pub enum DrizzleKernel {
    Square,
    Point,
    Gaussian { fwhm: f64 },
    Lanczos { order: u32 },
}
```

---

## 7. Quality Targets

| Metric | Target | Current Status |
|--------|--------|----------------|
| Registration accuracy | < 0.1 pixel RMS | Achieved |
| Centroid precision | < 0.05 pixel | Achieved |
| Processing speed | < 1 sec/frame (4K) | Partial (CPU) |
| Memory efficiency | < 8GB for 100 frames | Achieved |
| SNR improvement | Within 5% of theoretical | Achieved |

---

## 8. Testing Strategy

### Unit Tests [COMPLETE]
- Star detection accuracy on synthetic images
- Transformation estimation with known parameters
- Interpolation quality metrics
- Sigma clipping correctness

### Integration Tests [COMPLETE]
- Full pipeline on reference datasets
- Comparison with Siril/DSS output
- Regression tests for quality metrics

### Benchmark Datasets
- Deep sky: Orion Nebula, Andromeda Galaxy
- Planetary: Jupiter, Saturn (high frame rate)
- Solar: Sun in H-alpha
- Synthetic: Computer-generated test patterns

---

## 9. References

### Academic Papers
- Fruchter & Hook (2002) - "Drizzle: A Method for the Linear Reconstruction of Undersampled Images"
- Evangelidis & Psarakis (2008) - "Parametric Image Alignment using Enhanced Correlation Coefficient"
- Van Dokkum (2001) - "Cosmic-Ray Rejection by Laplacian Edge Detection" (LACosmic)
- Huang et al. (2020) - "Iterative Phase Correlation Algorithm for High-precision Subpixel Image Registration"
- Liu et al. (2025) - "Astronomical image denoising by self-supervised deep learning" (Nature Astronomy)

### Software References
- [PixInsight Documentation](https://pixinsight.com/doc/)
- [Siril Documentation](https://siril.readthedocs.io/)
- [DeepSkyStacker](http://deepskystacker.free.fr/)
- [HST Drizzle Documentation](https://hst-docs.stsci.edu/drizzpac/chapter-3-description-of-the-drizzle-algorithm)
- [Astro Pixel Processor](https://www.astropixelprocessor.com/)

### Rust Ecosystem
- `ndarray` - N-dimensional arrays
- `image` - Image processing
- `fitsio` / `fitrs` - FITS file handling
- `nalgebra` - Linear algebra
- `wgpu` - GPU compute
- `rayon` - Parallel processing
- `rustfft` - FFT (consider GPU alternative)

---

## 10. Recommended Next Steps

### Immediate (High Impact)
1. **Implement Weighted Mean Integration** - Essential for quality-based stacking
2. **Add Linear Fit Clipping** - Handles gradients better than sigma clipping
3. **Implement GESD Rejection** - Best for large stacks (>50 frames)

### Short Term
4. **Drizzle Implementation** - Super-resolution capability
5. **GPU FFT** - Major performance improvement for phase correlation
6. **Local Normalization** - Required for multi-session data

### Medium Term
7. **Comet/Asteroid Mode** - Dual-stack approach
8. **Astrometric Solution** - Gaia catalog matching
9. **Full GPU Pipeline** - End-to-end acceleration

### Long Term
10. **Deep Learning Integration** - Self-supervised denoising
11. **Real-time Preview** - Live stacking capability

---

## 11. Success Criteria

The implementation will be considered successful when:

1. **Accuracy**: Registration achieves <0.1 pixel RMS error [ACHIEVED]
2. **Quality**: Stacked output matches or exceeds Siril/DSS quality [ACHIEVED]
3. **Performance**: GPU-accelerated pipeline processes 4K frames at >1 fps [PARTIAL]
4. **Robustness**: Handles satellite trails, cosmic rays, varying conditions [ACHIEVED]
5. **Usability**: Clean API with sensible defaults [ACHIEVED]

---

*Document Version: 2.0*
*Created: January 2025*
*Updated: January 2026*

---

## Sources

- [Siril Stacking Documentation](https://siril.readthedocs.io/en/stable/preprocessing/stacking.html)
- [HST Drizzle Concept](https://hst-docs.stsci.edu/drizzpac/chapter-3-description-of-the-drizzle-algorithm/3-2-drizzle-concept)
- [Iterative Phase Correlation for High-precision Registration](https://ui.adsabs.harvard.edu/abs/2020ApJS..247....8H/abstract)
- [Image Registration - FreeAstro](https://free-astro.org/index.php?title=Image_registration)
- [Astronomical Image Denoising - Nature Astronomy 2025](https://www.nature.com/articles/s41550-025-02484-z)
- [Light Vortex PixInsight Tutorial](https://www.lightvortexastronomy.com/tutorial-pre-processing-calibrating-and-stacking-images-in-pixinsight.html)
- [Astro Pixel Processor Features](https://www.astropixelprocessor.com/)
