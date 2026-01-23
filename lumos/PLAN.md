# Astrophotography Image Stacking Implementation Plan

## Overview

This document outlines a comprehensive plan for implementing state-of-the-art astrophotography image stacking with sub-pixel tracking. The implementation will combine the best algorithms from leading software like PixInsight, Siril, and DeepSkyStacker, while leveraging GPU acceleration for performance.

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

### 2.1 Image I/O Module

**Purpose**: Load and save astronomical image formats with full metadata support.

**Supported Formats**:
- FITS (Flexible Image Transport System) - primary format
- RAW camera formats (via rawloader or similar)
- TIFF (16-bit, 32-bit float)
- PNG (8-bit, 16-bit)

**Key Features**:
- Parse FITS headers for exposure time, temperature, gain, date/time
- Support for monochrome and Bayer-pattern sensors
- Memory-mapped file access for large images
- Batch loading with parallel I/O

**Rust Crates**:
- `fitsio` or `fitrs` for FITS files
- `rawloader` for camera RAW files
- `image` crate for standard formats

---

### 2.2 Calibration Module

**Purpose**: Remove systematic errors from light frames using calibration frames.

#### 2.2.1 Calibration Frame Types

| Frame Type | Description | Capture Method |
|------------|-------------|----------------|
| **Bias** | Read noise pattern | Shortest exposure, cap on |
| **Dark** | Thermal noise pattern | Same exposure/temp as lights, cap on |
| **Flat** | Optical vignetting & dust | Even illumination, same optical path |
| **Flat-Dark** | Dark for flat exposure | Same exposure as flats, cap on |

#### 2.2.2 Calibration Pipeline

```rust
// Pseudo-code for calibration
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

**Algorithm**: Sigma-clipped mean with outlier rejection

```
For each pixel position (x, y):
    1. Collect values from all frames
    2. Calculate median and standard deviation
    3. Reject values > κσ from median (κ = 2.5-3.0)
    4. Average remaining values
    5. Store in master frame
```

#### 2.2.4 Hot/Cold Pixel Detection & Correction

**Methods**:
1. **From dark frames**: Identify pixels with values > 5σ above median
2. **Laplacian detection (LACosmic)**: Detect cosmic rays and hot pixels via edge detection
3. **Temporal outlier rejection**: Pixels that are outliers across the frame stack

**Correction**: Replace bad pixels with median of 8-connected neighbors (or weighted Gaussian interpolation).

---

### 2.3 Star Detection Module

**Purpose**: Detect stars and compute sub-pixel accurate centroids.

#### 2.3.1 Detection Algorithm

**Multi-scale approach**:

1. **Background estimation**: 
   - Divide image into tiles (e.g., 64x64 pixels)
   - Compute sigma-clipped median per tile
   - Interpolate to create smooth background map
   - Subtract background from image

2. **Initial detection**:
   - Apply threshold: pixel > background + k×σ (k ≈ 3-5)
   - Connected component labeling to group pixels
   - Filter by size (reject too small/large)

3. **Candidate filtering**:
   - Reject elongated objects (eccentricity > threshold)
   - Reject objects near edges
   - Reject saturated stars

#### 2.3.2 Sub-Pixel Centroid Computation

**Method 1: Gaussian PSF Fitting** (recommended for accuracy)

```
For each detected star:
    1. Extract stamp (e.g., 15x15 pixels centered on peak)
    2. Fit 2D Gaussian: G(x,y) = A·exp(-((x-x₀)²/2σx² + (y-y₀)²/2σy²)) + B
    3. Parameters: amplitude (A), centroid (x₀, y₀), widths (σx, σy), background (B)
    4. Use Levenberg-Marquardt optimization
    5. Centroid accuracy: ~0.01-0.1 pixel
```

**Method 2: Center of Mass (faster, less accurate)**

```
x_centroid = Σ(x · I(x,y)) / Σ(I(x,y))
y_centroid = Σ(y · I(x,y)) / Σ(I(x,y))
```

**Method 3: Iterative Weighted Centroid**

```
1. Initial guess from brightest pixel
2. Compute weighted centroid with Gaussian weights
3. Re-center window on new centroid
4. Repeat until convergence (typically 3-5 iterations)
5. Achieves ~0.05 pixel accuracy
```

#### 2.3.3 PSF Modeling

**Moffat Profile** (better than Gaussian for real optics):

```
I(r) = I₀ · (1 + (r/α)²)^(-β)

Where:
- r = distance from center
- α = core width parameter  
- β = wing power (typically 2.5-4.5)
- β → ∞ approaches Gaussian
```

**Empirical PSF** (highest accuracy):
1. Select 10-20 brightest unsaturated stars
2. Extract and normalize stamps
3. Align to sub-pixel precision
4. Stack to create oversampled PSF model

---

### 2.4 Image Registration Module

**Purpose**: Align all frames to a reference with sub-pixel accuracy.

#### 2.4.1 Coarse Registration: Triangle Matching

**Algorithm** (used by Siril, ASTAP):

```
1. Select N brightest stars (N ≈ 20-50) from reference image
2. Form all possible triangles from these stars
3. For each triangle, compute invariant descriptors:
   - Ratio of side lengths (scale-invariant)
   - Angles (rotation-invariant)
4. Build hash table of triangle descriptors

5. For each target image:
   a. Detect stars and form triangles
   b. Match triangles by descriptor similarity
   c. Accumulate votes for star correspondences
   d. Select matches with most votes
```

**RANSAC Refinement**:
```
1. Randomly select minimal set of matches (3 for affine, 4 for homography)
2. Compute transformation from selected matches
3. Count inliers (matches consistent with transformation)
4. Repeat N times, keep best transformation
5. Re-estimate using all inliers
```

#### 2.4.2 Fine Registration: Sub-Pixel Methods

**Method 1: Phase Correlation with Sub-Pixel Refinement**

```
1. Compute FFT of reference (F) and target (G) images
2. Cross-power spectrum: R = F* · G / |F* · G|
3. Inverse FFT → correlation surface
4. Find peak location
5. Sub-pixel refinement via:
   - Parabolic interpolation (fast, ~0.1 px accuracy)
   - Gaussian fitting (slower, ~0.01 px accuracy)
```

**Method 2: Iterative Phase Correlation (IPC)**

State-of-the-art for solar/astronomical imaging (0.01-0.001 pixel accuracy):

```
1. Initial estimate from standard phase correlation
2. Shift target image by current estimate
3. Recompute phase correlation on shifted image
4. Update estimate with residual shift
5. Repeat until convergence (typically 3-10 iterations)
```

**Method 3: ECC (Enhanced Correlation Coefficient)**

```
1. Define motion model (translation, Euclidean, affine, homography)
2. Initialize transformation matrix
3. Iteratively optimize to maximize:
   ECC = corr(template, warp(image, W))
4. Robust to illumination changes
5. Available in OpenCV: cv::findTransformECC()
```

**Method 4: Optical Flow (Lucas-Kanade)**

For local deformation correction:
```
1. Assume small motion and constant brightness
2. Solve: Ix·u + Iy·v + It = 0
3. Use spatial neighborhood for over-determined system
4. Provides dense displacement field
```

#### 2.4.3 Transformation Models

| Model | DOF | Parameters | Use Case |
|-------|-----|------------|----------|
| Translation | 2 | tx, ty | Tracked mount, small FOV |
| Euclidean | 3 | tx, ty, θ | Alt-az mount field rotation |
| Similarity | 4 | tx, ty, θ, s | Scale changes (focus drift) |
| Affine | 6 | 2×3 matrix | Perspective-free distortion |
| Homography | 8 | 3×3 matrix | Wide field, perspective |

#### 2.4.4 Interpolation for Sub-Pixel Warping

**Lanczos-3** (recommended - best quality):
```
L(x) = sinc(x) · sinc(x/3)  for |x| < 3
     = 0                     otherwise

Weight = L(dx) · L(dy)
```

**Bicubic** (good quality, faster):
```
Uses cubic polynomial interpolation
4×4 pixel neighborhood
```

**Bilinear** (fast, lower quality):
```
Linear interpolation
2×2 pixel neighborhood
```

---

### 2.5 Stacking Module

**Purpose**: Combine registered frames to maximize SNR and reject artifacts.

#### 2.5.1 Normalization Methods

**Global Normalization**:
```
For each frame:
    scale = reference_median / frame_median
    offset = reference_background - frame_background × scale
    normalized = frame × scale + offset
```

**Local Normalization** (PixInsight-style):
```
1. Divide image into tiles (e.g., 128×128 pixels)
2. For each tile:
   - Compute local median and scale factor
   - Interpolate between tiles for smooth correction
3. Handles varying sky gradients across sessions
```

#### 2.5.2 Pixel Rejection Algorithms

**Sigma Clipping** (Kappa-Sigma):
```
Input: pixel stack P[1..N], κ (typically 2.5-3.0)
Repeat:
    μ = mean(P)
    σ = stddev(P)
    Remove pixels where |P[i] - μ| > κ·σ
Until no changes or max iterations
Return: mean of remaining pixels
```

**Winsorized Sigma Clipping**:
```
Same as sigma clipping, but replace outliers with 
the boundary value (μ ± κσ) instead of removing them.
More robust for small sample sizes.
```

**Linear Fit Clipping**:
```
1. Fit linear relationship between each pixel and reference
2. Reject pixels that deviate significantly from fit
3. Better handles non-linear response variations
```

**Percentile Clipping** (for small stacks, N < 10):
```
Reject the highest and lowest P% of values
Typically P = 10-20%
```

**Generalized Extreme Studentized Deviate (ESD)**:
```
Statistical test for multiple outliers
More rigorous than simple sigma clipping
Better for identifying multiple artifacts
```

#### 2.5.3 Integration Methods

**Mean** (maximum SNR when no outliers):
```
output[x,y] = (1/N) × Σ frame[i][x,y]
SNR improvement: √N
```

**Median** (robust to outliers, lower SNR):
```
output[x,y] = median(frame[1..N][x,y])
SNR improvement: √(π/2) × √N ≈ 0.8√N
```

**Weighted Mean** (optimal with varying quality):
```
output[x,y] = Σ(w[i] × frame[i][x,y]) / Σ(w[i])

Weights based on:
- Frame SNR (noise estimation)
- FWHM (seeing quality)
- Eccentricity (tracking quality)
- Background level
```

#### 2.5.4 Drizzle Integration (Super-Resolution)

**Purpose**: Recover resolution lost to undersampling when dithering was used.

**Algorithm**:
```
1. Create output grid at higher resolution (e.g., 2× or 3×)
2. For each input frame:
   a. For each input pixel:
      - Shrink pixel to "droplet" (drop_size < 1.0)
      - Map droplet corners to output grid using transformation
      - Distribute flux to output pixels proportionally to overlap
3. Normalize by coverage map
```

**Parameters**:
- **Scale**: Output resolution multiplier (1.5×, 2×, 3×)
- **Drop size (pixfrac)**: Shrink factor (0.5-0.9 typical)
- **Kernel**: Square, circular, Gaussian, Lanczos

**Requirements**:
- Dithered input frames (random sub-pixel offsets between frames)
- Many frames (>50 recommended)
- Low eccentricity (good tracking)

---

### 2.6 GPU Acceleration

**Purpose**: Achieve real-time or near-real-time processing.

#### 2.6.1 GPU-Accelerated Components

| Component | CPU Time | GPU Speedup | Method |
|-----------|----------|-------------|--------|
| FFT (1024²) | 50ms | 10-50× | cuFFT |
| Interpolation | 100ms | 20-100× | Texture sampling |
| Sigma clipping | 200ms | 30-50× | Parallel reduction |
| Star detection | 150ms | 10-20× | Parallel threshold |
| Convolution | 80ms | 20-40× | Shared memory |

#### 2.6.2 Implementation Options

**Option A: wgpu (Cross-platform, recommended)**
```rust
// Compute shader for sigma clipping
@compute @workgroup_size(256)
fn sigma_clip(
    @builtin(global_invocation_id) id: vec3<u32>,
    @storage(read) pixels: array<f32>,
    @storage(read_write) output: array<f32>,
    @uniform params: ClipParams
) {
    // Parallel sigma clipping implementation
}
```

**Option B: CUDA (NVIDIA only, maximum performance)**
- Use cuFFT for FFT operations
- Custom kernels for pixel operations
- Thrust for parallel algorithms

**Option C: OpenCL (Cross-vendor)**
- Portable across GPU vendors
- Slightly lower performance than CUDA

#### 2.6.3 Memory Management

```
Strategy for large images:
1. Tile-based processing for images > GPU memory
2. Asynchronous data transfer (overlap compute/transfer)
3. Memory pools for frame buffers
4. Double buffering for continuous processing
```

---

## 3. Advanced Features

### 3.1 Automatic Quality Weighting

**SubframeSelector (PixInsight-style)**:

Compute quality metrics per frame:
```
- FWHM: Full Width at Half Maximum of stars
- Eccentricity: Star elongation (1.0 = round)
- SNR: Signal-to-noise ratio estimate
- Stars: Number of detected stars
- Background: Sky background level
- Noise: Background noise estimate
```

**Weighting function**:
```
weight = (SNR^a × (1/FWHM)^b × (1/eccentricity)^c) / noise

Typical exponents: a=1, b=2, c=1
```

### 3.2 Comet/Asteroid Stacking Mode

**Dual-stack approach**:
1. Stack on stars (comet trails)
2. Stack on comet (stars trail)
3. Optionally combine: stars from (1), comet from (2)

**Comet tracking**:
```
1. User provides comet positions at t₁ and t₂
2. Compute motion vector
3. Apply frame-specific offset based on timestamp
```

### 3.3 Multi-Session Integration

**Challenge**: Combining data from different nights with varying conditions.

**Solution**:
1. Local normalization to match backgrounds
2. Per-session quality assessment
3. Session-weighted integration
4. Gradient removal post-stack

### 3.4 Distortion Correction

**Optical distortion models**:
- Radial (barrel/pincushion): r' = r(1 + k₁r² + k₂r⁴)
- Tangential: from decentered lens elements
- Field curvature: focus varies across field

**Astrometric solution**:
1. Match detected stars to catalog (Gaia, UCAC4)
2. Fit distortion model
3. Create distortion map
4. Apply during registration

---

## 4. Implementation Phases

### Phase 1: Core Foundation
- [ ] Image I/O (FITS, TIFF, RAW)
- [ ] Basic calibration (bias, dark, flat subtraction)
- [ ] Hot pixel detection and removal
- [ ] Star detection with centroid computation
- [ ] Basic registration (translation only)
- [ ] Simple mean/median stacking

### Phase 2: Sub-Pixel Registration
- [ ] Triangle matching algorithm
- [ ] RANSAC transformation estimation
- [ ] Phase correlation with sub-pixel refinement
- [ ] Lanczos interpolation
- [ ] Affine/homography transformations

### Phase 3: Advanced Stacking
- [ ] Sigma clipping and variants
- [ ] Weighted integration
- [ ] Local normalization
- [ ] Quality metrics computation
- [ ] Automatic frame weighting

### Phase 4: Super-Resolution
- [ ] Drizzle integration
- [ ] Multiple kernel support
- [ ] Automatic dither detection

### Phase 5: GPU Acceleration
- [ ] GPU FFT for phase correlation
- [ ] GPU interpolation
- [ ] GPU pixel rejection
- [ ] Batch processing pipeline

### Phase 6: Polish & Advanced Features
- [ ] Comet stacking mode
- [ ] Distortion correction
- [ ] Astrometric solution
- [ ] Multi-session support
- [ ] Real-time preview

---

## 5. Data Structures

```rust
/// Core image representation
#[derive(Debug)]
pub struct AstroImage {
    pub data: ndarray::Array2<f32>,  // Or f64 for high precision
    pub width: u32,
    pub height: u32,
    pub metadata: ImageMetadata,
}

#[derive(Debug)]
pub struct ImageMetadata {
    pub exposure_time: f64,           // seconds
    pub temperature: Option<f64>,     // Celsius
    pub gain: Option<u32>,
    pub timestamp: DateTime<Utc>,
    pub filter: Option<String>,
    pub bayer_pattern: Option<BayerPattern>,
}

/// Detected star with sub-pixel position
#[derive(Debug)]
pub struct Star {
    pub x: f64,                       // Sub-pixel X centroid
    pub y: f64,                       // Sub-pixel Y centroid
    pub flux: f64,                    // Integrated flux
    pub fwhm: f64,                    // Full width half maximum
    pub eccentricity: f64,            // Elongation measure
    pub snr: f64,                     // Signal to noise ratio
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
    pub drop_size: f64,    // Pixel fraction (0.5-0.9)
    pub kernel: DrizzleKernel,
}
```

---

## 6. Quality Targets

| Metric | Target | Measurement Method |
|--------|--------|-------------------|
| Registration accuracy | < 0.1 pixel RMS | Synthetic test with known shifts |
| Centroid precision | < 0.05 pixel | Comparison to PSF fitting |
| Processing speed | < 1 sec/frame (4K) | Benchmark on RTX 3080 |
| Memory efficiency | < 8GB for 100 frames | Peak memory profiling |
| SNR improvement | Within 5% of theoretical | √N comparison |

---

## 7. Testing Strategy

### Unit Tests
- Star detection accuracy on synthetic images
- Transformation estimation with known parameters
- Interpolation quality metrics
- Sigma clipping correctness

### Integration Tests
- Full pipeline on reference datasets
- Comparison with Siril/DSS output
- Regression tests for quality metrics

### Benchmark Datasets
- Deep sky: Orion Nebula, Andromeda Galaxy
- Planetary: Jupiter, Saturn (high frame rate)
- Solar: Sun in H-alpha
- Synthetic: Computer-generated test patterns

---

## 8. References

### Academic Papers
- Fruchter & Hook (2002) - "Drizzle: A Method for the Linear Reconstruction of Undersampled Images"
- Evangelidis & Psarakis (2008) - "Parametric Image Alignment using Enhanced Correlation Coefficient"
- Van Dokkum (2001) - "Cosmic-Ray Rejection by Laplacian Edge Detection" (LACosmic)
- Huang et al. (2020) - "Iterative Phase Correlation Algorithm for High-precision Subpixel Image Registration"

### Software References
- [PixInsight Documentation](https://pixinsight.com/doc/)
- [Siril Documentation](https://siril.readthedocs.io/)
- [DeepSkyStacker](http://deepskystacker.free.fr/)
- [AutoStakkert](https://www.autostakkert.com/)
- [Astropy CCD Reduction Guide](https://www.astropy.org/ccd-reduction-and-photometry-guide/)

### Rust Ecosystem
- `ndarray` - N-dimensional arrays
- `image` - Image processing
- `fitsio` / `fitrs` - FITS file handling
- `nalgebra` - Linear algebra
- `wgpu` - GPU compute
- `rayon` - Parallel processing

---

## 9. Success Criteria

The implementation will be considered successful when:

1. **Accuracy**: Registration achieves <0.1 pixel RMS error
2. **Quality**: Stacked output matches or exceeds Siril/DSS quality
3. **Performance**: GPU-accelerated pipeline processes 4K frames at >1 fps
4. **Robustness**: Handles satellite trails, cosmic rays, varying conditions
5. **Usability**: Clean API with sensible defaults

---

*Document Version: 1.0*
*Created: January 2026*
