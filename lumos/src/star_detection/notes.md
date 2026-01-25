# Star Detection Algorithm Review (Updated)

## Current Implementation Status

The following features from the original review have been **implemented**:

- ✅ Matched filtering / Gaussian convolution before thresholding
- ✅ Tile grid median filtering in background estimation
- ✅ Reduced dilation radius (now radius=1)
- ✅ Sharpness metric for cosmic ray rejection
- ✅ Adaptive centroid window size based on expected FWHM
- ✅ Simple deblending for multi-peak components (local maxima detection)
- ✅ Configurable parameters (deblend settings, duplicate separation, sharpness threshold)
- ✅ Diagnostic output (StarDetectionDiagnostics struct)
- ✅ Roundness metrics (DAOFIND style SROUND/GROUND)
- ✅ L.A.Cosmic algorithm for cosmic ray detection (Laplacian SNR metric)
- ✅ Multi-threshold deblending (SExtractor-style tree-based deblending)
- ✅ Iterative background refinement (SExtractor-style object masking)
- ✅ 2D Gaussian fitting for high-precision centroids (Levenberg-Marquardt)
- ✅ 2D Moffat profile fitting (better PSF model for atmospheric seeing)

This review focuses on **remaining improvements** and **new insights** from published literature.

---

## 1. Background Estimation

### Current State
- Tiled sigma-clipped median with 3×3 tile median filter
- Bilinear interpolation between tiles
- MAD × 1.4826 for robust σ estimation
- Iterative background refinement with object masking (optional)

### Remaining Issues

#### ~~Issue 1: No iterative background refinement~~ IMPLEMENTED

Iterative background refinement is now available via `estimate_background_iterative()`.
Uses the SExtractor-style approach:
1. Estimate initial background
2. Detect pixels above threshold (potential objects)
3. Create dilated mask to cover object wings
4. Re-estimate background excluding masked pixels

Configuration via `IterativeBackgroundConfig`:
- `detection_sigma`: Threshold for masking (default: 3.0)
- `iterations`: Number of refinement iterations (default: 1)
- `mask_dilation`: Dilation radius for object masks (default: 3 pixels)
- `min_unmasked_fraction`: Minimum unmasked pixels per tile (default: 0.3)

#### Issue 2: Mode estimation for crowded fields

SExtractor switches between mean and mode based on field crowding:
- If σ changes < 20% during clipping → uncrowded → use clipped mean
- Otherwise → crowded → use mode = 2.5 × median - 1.5 × mean

Reference: [Bertin & Arnouts 1996](https://aas.aanda.org/articles/aas/abs/1996/08/ds1060/ds1060.html)

#### Issue 3: Variable background mesh size

For images with strong gradients or nebulosity, smaller tiles may be needed in some regions. An adaptive tile size based on local gradient strength could improve results.

---

## 2. Detection Algorithm

### Current State
- Matched filter (Gaussian convolution) for SNR boost
- Threshold on convolved image
- Morphological dilation (radius=1)
- Connected component labeling with union-find
- Simple deblending via local maxima detection
- Multi-threshold deblending (SExtractor-style, optional)

### Remaining Issues

#### ~~Issue 1: No multi-threshold deblending (Major)~~ IMPLEMENTED

Multi-threshold deblending is now available via `multi_threshold_deblend: true` in config.
Uses exponentially-spaced thresholds between detection level and peak, builds a tree
structure, and applies contrast criterion (DEBLEND_MINCONT equivalent) to decide splits.

Configuration parameters:
- `deblend_nthresh`: Number of sub-thresholds (default: 32)
- `deblend_min_contrast`: Minimum contrast for branch splitting (default: 0.005)

#### Issue 2: No handling of diffraction spikes

Bright stars produce diffraction spikes that can be detected as separate objects or cause false positives. Modern pipelines mask or model these artifacts.

#### Issue 3: Detection threshold could be adaptive

Different regions of an image may benefit from different detection thresholds. Faint nebulae regions need higher thresholds to avoid false positives.

---

## 3. Centroid Computation

### Current State
- Iterative Gaussian-weighted centroid
- Adaptive stamp radius (~3.5× FWHM)
- Convergence threshold of 0.001 pixels²
- Sharpness metric (peak/core flux)

### Remaining Issues

#### ~~Issue 1: Gaussian fitting would be more accurate~~ IMPLEMENTED

2D Gaussian fitting is now available via `fit_gaussian_2d()` in `gaussian_fit.rs`.
Uses Levenberg-Marquardt optimization to fit:
```
f(x,y) = A × exp(-((x-x₀)²/2σₓ² + (y-y₀)²/2σᵧ²)) + B
```

Returns `GaussianFitResult` with sub-pixel position (~0.01 pixel accuracy), sigmas, and convergence info.
Configuration via `GaussianFitConfig` (max iterations, convergence threshold, L-M damping parameters).

Reference: [Sub-pixel centroid algorithms](https://link.springer.com/article/10.1007/s11554-014-0408-z)

#### ~~Issue 2: No Moffat profile support~~ IMPLEMENTED

2D Moffat profile fitting is now available via `fit_moffat_2d()` in `moffat_fit.rs`.
The Moffat profile better matches atmospheric seeing with extended wings:
```
I(r) = I₀ × (1 + (r/α)²)^(-β) + B
```

Supports both fixed beta (more robust, 5 parameters) and variable beta (6 parameters) fitting.
Returns `MoffatFitResult` with position, alpha, beta, FWHM, and convergence info.
Configuration via `MoffatFitConfig` (fit_beta toggle, fixed_beta value, L-M parameters).

Conversion functions: `alpha_beta_to_fwhm()` and `fwhm_beta_to_alpha()`.

Reference: [Moffat 1969, A&A](https://ui.adsabs.harvard.edu/abs/1969A&A.....3..455M)

#### Issue 3: SNR formula ignores Poisson noise

Current: `SNR = flux / (σ_sky × √npix)`

This is correct for background-dominated regime but underestimates error for bright stars. Full CCD equation:

```
SNR = flux / √(flux/gain + npix × (σ_sky² + σ_read²/gain²))
```

For image registration (not photometry), this is a minor issue.

---

## 4. Quality Metrics and Filtering

### Current State
- Sharpness for cosmic ray rejection
- Eccentricity for shape filtering
- FWHM outlier rejection (MAD-based)
- Duplicate removal
- L.A.Cosmic Laplacian SNR metric for cosmic ray detection
- Roundness metrics (DAOFIND GROUND and SROUND)

### Remaining Issues

#### ~~Issue 1: L.A.Cosmic algorithm for cosmic rays~~ IMPLEMENTED

L.A.Cosmic Laplacian SNR metric is now computed for each star in `Star.laplacian_snr`.
Uses second-derivative edge detection to identify cosmic rays (sharp edges → high Laplacian).
The `is_cosmic_ray_laplacian(threshold)` method allows filtering. Typical threshold: ~50.

Implementation in `cosmic_ray.rs`:
- `compute_laplacian()`: 3x3 Laplacian convolution
- `compute_fine_structure()`: 3x3 median for noise estimation  
- `compute_laplacian_snr()`: Per-star metric computation

Reference: [van Dokkum 2001, PASP](https://ui.adsabs.harvard.edu/abs/2001PASP..113.1420V)

#### ~~Issue 2: No roundness metric (DAOFIND style)~~ IMPLEMENTED

Roundness metrics are now computed for each star:
- `Star.roundness1` (GROUND): `(Hx - Hy) / (Hx + Hy)` from marginal Gaussian fits
- `Star.roundness2` (SROUND): Bilateral vs four-fold symmetry ratio

The `is_round(max_roundness)` method allows filtering non-circular sources.
Config parameter `max_roundness` controls acceptance threshold (default: 1.0 = disabled).

#### Issue 3: No star/galaxy separation

For deep images, faint galaxies may be detected as stars. SExtractor uses a neural network for classification. Simpler approaches:

- Compare measured FWHM to stellar FWHM
- Use concentration index (flux ratio in different apertures)
- Surface brightness profile analysis

---

## 5. PSF Photometry (Missing Feature)

### Why It Matters

For crowded fields (star clusters, galactic bulge), aperture photometry fails because stellar profiles overlap. PSF photometry is essential:

1. Build PSF model from isolated stars
2. Simultaneously fit PSF to all stars
3. Subtract fitted stars to find fainter ones (IterativePSFPhotometry)

### Implementation Approach

```rust
pub struct PSFModel {
    // Empirical PSF (ePSF) from stacked isolated stars
    // or analytic model (Gaussian, Moffat)
}

pub fn psf_photometry(
    pixels: &[f32],
    stars: &[Star],
    psf: &PSFModel,
) -> Vec<PSFResult> {
    // Simultaneous least-squares fit of PSF to all stars
    // Returns refined positions and fluxes
}
```

Reference: [Stetson 1987 DAOPHOT paper](https://ui.adsabs.harvard.edu/abs/1987PASP...99..191S)

---

## 6. Wavelet-Based Detection (Alternative Approach)

### Concept

Wavelet transforms provide multi-scale decomposition naturally suited for detecting objects at different sizes:

1. Decompose image into wavelet scales
2. Each scale emphasizes features of specific size
3. Detect objects at their "natural" scale
4. Combine detections across scales

### Advantages

- Better separation of stars from extended sources (galaxies, nebulae)
- Natural handling of varying PSF across field
- More robust to complex backgrounds

### Disadvantage

- More complex implementation
- Harder to tune parameters

Reference: [Starlet Transform](http://jstarck.free.fr/Chapter_Starlet2011.pdf)

---

## 7. Deep Learning Approaches (Future)

### CNN-Based Star Detection

Recent papers show CNNs can outperform classical methods:

- **PNet**: End-to-end photometry with uncertainty estimation
- **YOLO for stars**: Real-time detection with modern object detectors
- **Neural centroiding**: Order of magnitude improvement over CoG

### Advantages

- Robust to complex noise patterns
- Can learn to reject artifacts
- Handles non-Gaussian PSFs naturally

### Disadvantages

- Requires training data
- "Black box" behavior
- Computational cost (though inference is fast)

Reference: [CNN star detection](https://arxiv.org/html/2404.19108v1)

---

## 8. Remaining Implementation Priorities

| Priority | Feature | Impact | Complexity | Status |
|----------|---------|--------|------------|--------|
| ~~**High**~~ | ~~Multi-threshold deblending~~ | ~~Better crowded field handling~~ | ~~Medium~~ | ✅ Done |
| ~~**High**~~ | ~~Roundness metric (DAOFIND style)~~ | ~~Reject more artifacts~~ | ~~Low~~ | ✅ Done |
| ~~**Medium**~~ | ~~L.A.Cosmic for cosmic rays~~ | ~~More robust CR rejection~~ | ~~Medium~~ | ✅ Done |
| ~~**Medium**~~ | ~~Iterative background refinement~~ | ~~Cleaner background in crowded fields~~ | ~~Medium~~ | ✅ Done |
| ~~**Medium**~~ | ~~Optional Gaussian fitting~~ | ~~Higher centroid accuracy~~ | ~~Medium~~ | ✅ Done |
| ~~**Low**~~ | ~~Moffat profile support~~ | ~~Better PSF modeling~~ | ~~Low~~ | ✅ Done |
| **Low** | PSF photometry | Essential for crowded fields | High | Pending |
| **Low** | Wavelet detection | Alternative approach | High | Pending |

---

## 9. Known Limitations of Current Implementation

1. **No gain parameter**: SNR calculation assumes background-dominated regime
2. **No read noise handling**: Affects faint star detection accuracy
3. **Fixed CFA handling**: Median filter always applied, even for mono sensors
4. **No WCS awareness**: Cannot use catalog positions for verification
5. **No defect map support**: Hot columns, bad pixels not explicitly handled
6. **No focus quality metric**: Could estimate seeing from stellar FWHM distribution

---

## References

### Foundational Papers
- [Stetson 1987 - DAOPHOT](https://ui.adsabs.harvard.edu/abs/1987PASP...99..191S) - Crowded field photometry
- [Bertin & Arnouts 1996 - SExtractor](https://aas.aanda.org/articles/aas/abs/1996/08/ds1060/ds1060.html) - Source extraction
- [van Dokkum 2001 - L.A.Cosmic](https://ui.adsabs.harvard.edu/abs/2001PASP..113.1420V) - Cosmic ray rejection

### Modern Implementations
- [Photutils DAOStarFinder](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
- [Photutils IRAFStarFinder](https://photutils.readthedocs.io/en/stable/api/photutils.detection.IRAFStarFinder.html)
- [Astrometry.net](https://arxiv.org/abs/0910.2233) - Blind astrometric calibration

### Advanced Topics
- [SDSS Deblending](https://www.sdss4.org/dr17/algorithms/deblend/)
- [Starlet Transform](http://jstarck.free.fr/Chapter_Starlet2011.pdf) - Wavelet methods
- [Improved object detection](https://academic.oup.com/mnras/article/451/4/4445/1118406) - 2 mag fainter than SExtractor

### Centroid Accuracy
- [Sub-pixel centroid on FPGA](https://link.springer.com/article/10.1007/s11554-014-0408-z) - 1/33 pixel accuracy
- [Gaussian Analytic Centroiding](https://www.sciencedirect.com/science/article/abs/pii/S0273117715006110)
