# Centroid Computation: Best Practices and Implementation

This document covers sub-pixel centroid computation algorithms, quality metrics, and profile fitting techniques used in astronomical image processing.

## Table of Contents

1. [Overview](#overview)
2. [Centroid Algorithms](#centroid-algorithms)
3. [Profile Fitting](#profile-fitting)
4. [Quality Metrics](#quality-metrics)
5. [Implementation Details](#implementation-details)
6. [References](#references)

---

## Overview

Sub-pixel centroid computation is essential for precise astrometry and photometry. The goal is to determine the exact center of a stellar point spread function (PSF) with accuracy better than one pixel.

**Achievable Accuracy:**
- Weighted centroid methods: ~0.05-0.1 pixel
- Gaussian/Moffat fitting: ~0.01-0.02 pixel
- State-of-the-art (Kalman filtering): ~0.008 pixel

Our implementation uses a two-stage approach:
1. **Fast initial estimate** using iterative weighted centroid
2. **Optional refinement** using Gaussian or Moffat profile fitting

---

## Centroid Algorithms

### Center of Gravity (CoG)

The simplest approach calculates the intensity-weighted average position:

```
x_cog = Σ(x × I(x,y)) / Σ(I(x,y))
y_cog = Σ(y × I(x,y)) / Σ(I(x,y))
```

**Pros:** Simple, fast
**Cons:** Sensitive to noise, background bias, only integer accuracy

### Intensity Weighted Centroid (IWC)

Enhances CoG by squaring intensities, giving brighter pixels more influence:

```
x_iwc = Σ(x × I²(x,y)) / Σ(I²(x,y))
```

**Pros:** Better noise rejection than CoG
**Cons:** Still sensitive to background variations

### Iterative Weighted Centroid (Our Implementation)

Our algorithm uses Gaussian-weighted moments with iterative refinement:

```rust
// Weight pixels by Gaussian distance from current center
weight = value × exp(-dist² / (2σ²))

// Update centroid
new_cx = Σ(x × weight) / Σ(weight)
new_cy = Σ(y × weight) / Σ(weight)

// Iterate until convergence (typically 3-5 iterations)
```

**Key parameters:**
- `σ = 2.0` pixels (Gaussian weight sigma)
- Convergence threshold: 0.01 pixel
- Max iterations: 10

The algorithm converges quadratically, with each iteration approximately halving the error.

### Two-Step Extraction Method

A robust approach used in star trackers:

1. **Pixel-level detection:** Find maximum via zero-crossing of first derivative
2. **Sub-pixel refinement:** Apply weighted centroid in fixed window around peak

This method shows excellent noise resistance and processing speed.

---

## Profile Fitting

### Why Profile Fitting?

While weighted centroid is fast (~0.05 pixel accuracy), profile fitting achieves ~0.01 pixel accuracy by modeling the actual PSF shape.

### Gaussian Profile

The 2D Gaussian model:

```
f(x,y) = A × exp(-((x-x₀)²/2σ_x² + (y-y₀)²/2σ_y²)) + B
```

**Parameters:** x₀, y₀ (center), A (amplitude), σ_x, σ_y (widths), B (background)

**FWHM relationship:**
```
FWHM = 2.355 × σ
```

**Pros:** Good approximation for well-sampled PSFs
**Cons:** Doesn't model PSF wings accurately

### Moffat Profile

Better models atmospheric seeing with extended wings:

```
f(x,y) = A × (1 + r²/α²)^(-β) + B
where r² = (x-x₀)² + (y-y₀)²
```

**Parameters:**
- α (alpha): Core width parameter
- β (beta): Wing slope, typically 2.5-4.5 for ground-based seeing

**FWHM relationship:**
```
FWHM = 2α × sqrt(2^(1/β) - 1)
```

**Typical β values:**
- 4.765: Theoretical turbulence prediction (Trujillo et al.)
- 2.5: IRAF default, good for typical seeing
- 1.5: Large wings, poor seeing conditions

**Why Moffat over Gaussian?**
The Moffat function better reproduces the extended wings present in real stellar images caused by atmospheric turbulence. As β → ∞, the Moffat approaches a Gaussian.

### Levenberg-Marquardt Optimization

Both Gaussian and Moffat fitting use Levenberg-Marquardt (L-M) optimization:

```rust
// L-M update step
damped_hessian[i][i] *= (1 + λ)  // Add damping to diagonal
delta = solve(damped_hessian, gradient)
new_params = params + delta

if chi2(new_params) < chi2(params):
    params = new_params
    λ *= 0.1  // Reduce damping (more Newton-like)
else:
    λ *= 10   // Increase damping (more gradient descent-like)
```

**Key considerations:**
- Initial λ = 0.001
- Convergence when max parameter change < 1e-6
- Maximum 50 iterations
- Validate results (center within stamp, reasonable σ/α values)

---

## Quality Metrics

### DAOFIND-Style Metrics

Our implementation follows the DAOFIND algorithm (Stetson 1987) for quality assessment.

#### Sharpness

Measures how point-like a source is:

```
sharpness = peak_value / core_flux
```

Where `core_flux` is the sum in a 3×3 region around the peak.

**Interpretation:**
- 0.2-0.5: Normal stars
- ~1.0: Cosmic rays (most flux in single pixel)
- <0.2: Extended sources or blends

#### Roundness1 (GROUND)

Based on marginal Gaussian fit heights:

```
roundness1 = (Hx - Hy) / (Hx + Hy)
```

Where Hx, Hy are heights of best-fitting 1D Gaussians to x and y marginal distributions.

**Interpretation:**
- 0: Circular source
- Positive: Extended in y direction
- Negative: Extended in x direction

#### Roundness2 (SROUND)

Based on source symmetry:

```
asym_x = (sum_right - sum_left) / (sum_right + sum_left)
asym_y = (sum_bottom - sum_top) / (sum_bottom + sum_top)
roundness2 = sqrt(asym_x² + asym_y²)
```

**Interpretation:**
- 0: Perfectly symmetric
- >0.1: Asymmetric (possible cosmic ray trail, blend, or artifact)

### Signal-to-Noise Ratio (SNR)

Full CCD noise equation when gain is known:

```
SNR = flux / sqrt(flux/gain + npix × (σ_sky² + σ_read²/gain²))
```

Where:
- `flux`: Background-subtracted star flux (ADU)
- `gain`: Camera gain (e⁻/ADU)
- `npix`: Number of pixels in aperture
- `σ_sky`: Background noise (ADU)
- `σ_read`: Read noise (electrons)

Simplified background-dominated formula:

```
SNR = flux / (σ_sky × sqrt(npix))
```

### Eccentricity

Computed from the covariance matrix of weighted second moments:

```
cxx = Σ(I × (x-cx)²) / Σ(I)
cyy = Σ(I × (y-cy)²) / Σ(I)
cxy = Σ(I × (x-cx)(y-cy)) / Σ(I)

λ₁, λ₂ = eigenvalues of [[cxx, cxy], [cxy, cyy]]
eccentricity = sqrt(1 - λ₂/λ₁)
```

**Interpretation:**
- 0: Circular
- 0.3-0.5: Typical seeing-induced elongation
- >0.7: Trailing, bad tracking, or cosmic ray

### Laplacian SNR (L.A.Cosmic)

For cosmic ray detection, we compute the Laplacian SNR based on van Dokkum (2001):

```
laplacian = pixel - (1/4) × Σ(neighbors)
laplacian_snr = max(laplacian) / noise
```

Cosmic rays have sharp edges and high Laplacian SNR (typically >5-10), while stars have smooth profiles and lower values.

---

## Implementation Details

### Stamp Extraction

Stars are analyzed within a small stamp (cutout) around the peak:

```rust
stamp_radius = (expected_fwhm × 1.5).round() + 2
// Typical: 7-10 pixels for FWHM ~4 pixels
```

The stamp should be large enough to capture the PSF wings but small enough for efficient computation.

### Background Handling

Two methods supported:

1. **Global background map** (default, fastest): Uses pre-computed background per tile
2. **Local annulus**: Computes background from annular region around star (inner_r = stamp_radius, outer_r = 1.5 × stamp_radius)

Local annulus is more accurate for variable nebulosity but slower.

### Convergence Criteria

Iterative weighted centroid:
```rust
convergence_threshold = 0.01  // pixels
max_iterations = 10
```

Profile fitting (L-M):
```rust
convergence_threshold = 1e-6  // parameter change
max_iterations = 50
lambda_up = 10.0
lambda_down = 0.1
```

### Validation Checks

Results are rejected if:
- Center moved outside stamp radius
- σ or α outside reasonable range (0.5 to 2×stamp_radius)
- Zero or negative flux

---

## References

### Algorithms

1. **DAOFIND**: Stetson, P.B. 1987, PASP, 99, 191 - "DAOPHOT: A computer program for crowded-field stellar photometry"
   - [photutils DAOStarFinder](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
   - [IRAF daofind](https://iraf.readthedocs.io/en/latest/tasks/noao/digiphot/apphot/daofind.html)

2. **Moffat Profile**: Moffat, A.F.J. 1969, A&A, 3, 455
   - [GNU Astronomy Utilities - PSF](https://www.gnu.org/software/gnuastro/manual/html_node/PSF.html)
   - [Effects of seeing on Sérsic profiles](https://academic.oup.com/mnras/article/328/3/977/1247204)

3. **L.A.Cosmic**: van Dokkum, P.G. 2001, PASP, 113, 1420 - "Cosmic-Ray Rejection by Laplacian Edge Detection"
   - [ADS Abstract](https://ui.adsabs.harvard.edu/abs/2001PASP..113.1420V/abstract)
   - [lacosmic Python package](https://pypi.org/project/lacosmic/)

4. **Iterative Weighted Centroid**:
   - [Lost Infinity - Star Centroid with Sub-pixel Accuracy](https://www.lost-infinity.com/night-sky-image-processing-part-4-calculate-the-star-centroid-with-sub-pixel-accuracy/)
   - [Fast Gaussian Fitting for Star Sensors](https://www.ncbi.nlm.nih.gov/pmc/articles/PMC6163372/)

### Software Implementations

1. **photutils** (Python): [Documentation](https://photutils.readthedocs.io/)
2. **SEP** (Python/C): [GitHub](https://github.com/kbarbary/sep)
3. **Siril** (C/GTK): [PSF Documentation](https://free-astro.org/index.php/Siril:PSF)

### SNR and Photometry

1. Howell, S.B. 1989, PASP, 101, 616 - "Two-dimensional aperture photometry"
   - [Signal-to-noise considerations](http://spiff.rit.edu/classes/phys373/lectures/signal/signal.html)
   - [ESO Signal, Noise and Detection](https://www.eso.org/~ohainaut/ccd/sn.html)

---

## Module Structure

```
centroid/
├── mod.rs           # Main API: compute_centroid(), refine_centroid(), compute_metrics()
├── gaussian_fit.rs  # 2D Gaussian fitting with L-M optimization
├── moffat_fit.rs    # 2D Moffat fitting with L-M optimization
├── lm_optimizer.rs  # Shared Levenberg-Marquardt optimizer
├── linear_solver.rs # Generic NxN linear system solver
└── tests.rs         # Comprehensive test suite
```

### Public API

```rust
// Main centroid computation
pub fn compute_centroid(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    candidate: &StarCandidate,
    config: &StarDetectionConfig,
) -> Option<Star>;

// Profile fitting
pub fn fit_gaussian_2d(...) -> Option<GaussianFitResult>;
pub fn fit_moffat_2d(...) -> Option<MoffatFitResult>;

// Utility functions
pub fn alpha_beta_to_fwhm(alpha: f32, beta: f32) -> f32;
pub fn fwhm_beta_to_alpha(fwhm: f32, beta: f32) -> f32;
```
