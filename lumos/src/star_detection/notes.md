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

---

## 10. Code Refactoring & Optimization Plan

This section outlines the comprehensive refactoring plan to reorganize the star detection module
into a clean, testable, benchmarkable, and highly optimized architecture.

### 10.1 Target Directory Structure

```
star_detection/
├── mod.rs                    # Public API, Star struct, configs, find_stars()
├── notes.md                  # This file
│
├── background/               # Background estimation algorithms
│   ├── mod.rs               # Public interface, BackgroundMap
│   ├── tile_stats.rs        # Sigma-clipped statistics per tile
│   ├── interpolation.rs     # Bilinear interpolation
│   ├── iterative.rs         # Iterative refinement with masking
│   ├── simd/
│   │   ├── mod.rs           # SIMD dispatch
│   │   ├── sse.rs           # SSE/AVX implementation
│   │   └── neon.rs          # ARM NEON implementation
│   ├── tests.rs             # Unit tests
│   └── bench.rs             # Benchmarks
│
├── convolution/              # Matched filter / Gaussian convolution
│   ├── mod.rs               # Public interface
│   ├── separable.rs         # Separable 2D convolution
│   ├── simd/
│   │   ├── mod.rs
│   │   ├── sse.rs           # SIMD horizontal/vertical passes
│   │   └── neon.rs
│   ├── tests.rs
│   └── bench.rs
│
├── detection/                # Thresholding and connected components
│   ├── mod.rs               # Public interface, detect_stars()
│   ├── threshold.rs         # Adaptive thresholding
│   ├── union_find.rs        # Connected component labeling
│   ├── morphology.rs        # Dilation operations
│   ├── simd/
│   │   ├── mod.rs
│   │   ├── sse.rs           # SIMD threshold comparison
│   │   └── neon.rs
│   ├── tests.rs
│   └── bench.rs
│
├── deblend/                  # Star deblending algorithms
│   ├── mod.rs               # Public interface
│   ├── local_maxima.rs      # Simple deblending
│   ├── multi_threshold.rs   # SExtractor-style tree deblending
│   ├── tests.rs
│   └── bench.rs
│
├── centroid/                 # Centroid computation
│   ├── mod.rs               # Public interface
│   ├── weighted.rs          # Iterative weighted centroid
│   ├── gaussian_fit.rs      # 2D Gaussian L-M fitting
│   ├── moffat_fit.rs        # 2D Moffat L-M fitting
│   ├── linear_solver.rs     # 5x5/6x6 Gaussian elimination (shared)
│   ├── simd/
│   │   ├── mod.rs
│   │   ├── sse.rs           # SIMD weighted sums
│   │   └── neon.rs
│   ├── tests.rs
│   └── bench.rs
│
├── metrics/                  # Quality metrics computation
│   ├── mod.rs               # Public interface
│   ├── sharpness.rs         # Sharpness metric
│   ├── roundness.rs         # DAOFIND GROUND/SROUND
│   ├── eccentricity.rs      # Second moments
│   ├── fwhm.rs              # FWHM estimation
│   ├── tests.rs
│   └── bench.rs
│
├── cosmic_ray/               # Cosmic ray detection
│   ├── mod.rs               # Public interface
│   ├── laplacian.rs         # L.A.Cosmic Laplacian
│   ├── fine_structure.rs    # Median-based noise
│   ├── simd/
│   │   ├── mod.rs
│   │   ├── sse.rs           # SIMD 3x3 kernels
│   │   └── neon.rs
│   ├── tests.rs
│   └── bench.rs
│
├── median_filter/            # Median filtering
│   ├── mod.rs               # Public interface
│   ├── scalar.rs            # Scalar implementation
│   ├── simd/
│   │   ├── mod.rs
│   │   ├── sse.rs           # SIMD sorting networks
│   │   └── neon.rs
│   ├── tests.rs
│   └── bench.rs
│
└── filtering/                # Post-detection filtering
    ├── mod.rs               # Public interface
    ├── duplicates.rs        # Duplicate removal
    ├── fwhm_outliers.rs     # MAD-based FWHM filtering
    ├── quality.rs           # Combined quality filtering
    ├── tests.rs
    └── bench.rs
```

### 10.2 Phase 1: Module Reorganization

| Step | Task | Files Affected | Status |
|------|------|---------------|--------|
| 1.1 | Create `background/` folder structure | New folder | Pending |
| 1.2 | Move background code to `background/mod.rs`, `tile_stats.rs`, `interpolation.rs` | background/mod.rs → split | Pending |
| 1.3 | Move iterative refinement to `background/iterative.rs` | background/mod.rs → split | Pending |
| 1.4 | Add `background/tests.rs` (move existing tests) | background/tests.rs | Pending |
| 1.5 | Add `background/bench.rs` | New file | Pending |
| 1.6 | Create `convolution/` folder structure | New folder | Pending |
| 1.7 | Move matched_filter to `convolution/mod.rs`, `separable.rs` | convolution.rs → split | Pending |
| 1.8 | Add `convolution/tests.rs`, `convolution/bench.rs` | New files | Pending |
| 1.9 | Create `detection/` folder structure | New folder | Pending |
| 1.10 | Split detection into `threshold.rs`, `union_find.rs`, `morphology.rs` | detection/mod.rs → split | Pending |
| 1.11 | Add `detection/tests.rs`, `detection/bench.rs` | New files | Pending |
| 1.12 | Create `deblend/` folder structure | New folder | Pending |
| 1.13 | Move deblend code to `local_maxima.rs`, `multi_threshold.rs` | deblend.rs → split | Pending |
| 1.14 | Add `deblend/tests.rs`, `deblend/bench.rs` | New files | Pending |
| 1.15 | Create `centroid/` folder structure | New folder | Pending |
| 1.16 | Move centroid, gaussian_fit, moffat_fit into `centroid/` | Multiple files | Pending |
| 1.17 | Extract shared `linear_solver.rs` | New file | Pending |
| 1.18 | Add `centroid/tests.rs`, `centroid/bench.rs` | New files | Pending |
| 1.19 | Create `metrics/` folder structure | New folder | Pending |
| 1.20 | Split metrics into `sharpness.rs`, `roundness.rs`, `eccentricity.rs`, `fwhm.rs` | centroid.rs → split | Pending |
| 1.21 | Add `metrics/tests.rs`, `metrics/bench.rs` | New files | Pending |
| 1.22 | Create `cosmic_ray/` folder structure | New folder | Pending |
| 1.23 | Split cosmic_ray into `laplacian.rs`, `fine_structure.rs` | cosmic_ray.rs → split | Pending |
| 1.24 | Add `cosmic_ray/tests.rs`, `cosmic_ray/bench.rs` | New files | Pending |
| 1.25 | Create `median_filter/` folder structure | New folder | Pending |
| 1.26 | Move median_filter to `scalar.rs` | median_filter.rs → move | Pending |
| 1.27 | Add `median_filter/tests.rs`, `median_filter/bench.rs` | New files | Pending |
| 1.28 | Create `filtering/` folder structure | New folder | Pending |
| 1.29 | Extract filtering logic from mod.rs | mod.rs → split | Pending |
| 1.30 | Add `filtering/tests.rs`, `filtering/bench.rs` | New files | Pending |

### 10.3 Phase 2: Add SIMD Implementations

Modules that benefit from SIMD (data-parallel operations on pixel arrays):

| Module | SIMD Opportunity | Expected Speedup | Priority |
|--------|------------------|------------------|----------|
| **background** | Tile statistics (sum, sum_sq, count) | 3-4× | High |
| **convolution** | Separable kernel multiply-add | 4-8× | High |
| **detection** | Threshold comparison (packed compare) | 2-4× | Medium |
| **median_filter** | Sorting networks for 3x3 | 2-3× | High |
| **cosmic_ray** | 3x3 Laplacian convolution | 3-4× | Medium |
| **centroid** | Weighted sum accumulation | 2-3× | Low |
| **metrics** | Second moment computation | 2× | Low |

| Step | Task | Status |
|------|------|--------|
| 2.1 | Add `background/simd/mod.rs` with runtime dispatch | Pending |
| 2.2 | Implement `background/simd/sse.rs` (AVX2 tile stats) | Pending |
| 2.3 | Implement `background/simd/neon.rs` (NEON tile stats) | Pending |
| 2.4 | Benchmark background SIMD vs scalar | Pending |
| 2.5 | Add `convolution/simd/mod.rs` with runtime dispatch | Pending |
| 2.6 | Implement `convolution/simd/sse.rs` (AVX2 separable conv) | Pending |
| 2.7 | Implement `convolution/simd/neon.rs` (NEON separable conv) | Pending |
| 2.8 | Benchmark convolution SIMD vs scalar | Pending |
| 2.9 | Add `median_filter/simd/mod.rs` with runtime dispatch | Pending |
| 2.10 | Implement `median_filter/simd/sse.rs` (sorting networks) | Pending |
| 2.11 | Implement `median_filter/simd/neon.rs` | Pending |
| 2.12 | Benchmark median_filter SIMD vs scalar | Pending |
| 2.13 | Add `cosmic_ray/simd/mod.rs` with runtime dispatch | Pending |
| 2.14 | Implement `cosmic_ray/simd/sse.rs` (3x3 Laplacian) | Pending |
| 2.15 | Implement `cosmic_ray/simd/neon.rs` | Pending |
| 2.16 | Benchmark cosmic_ray SIMD vs scalar | Pending |
| 2.17 | Add `detection/simd/mod.rs` (threshold only) | Pending |
| 2.18 | Implement `detection/simd/sse.rs` (packed compare) | Pending |
| 2.19 | Implement `detection/simd/neon.rs` | Pending |
| 2.20 | Benchmark detection SIMD vs scalar | Pending |

### 10.4 Phase 3: Run Benchmarks & Profile

| Step | Task | Status |
|------|------|--------|
| 3.1 | Create comprehensive benchmark suite for all modules | Pending |
| 3.2 | Run benchmarks on x86_64 (Intel/AMD) | Pending |
| 3.3 | Run benchmarks on aarch64 (Apple Silicon / ARM server) | Pending |
| 3.4 | Profile with `perf` / Instruments for hotspots | Pending |
| 3.5 | Generate flamegraphs for full pipeline | Pending |
| 3.6 | Document baseline performance numbers | Pending |

### 10.5 Phase 4: Optimize - False Cache Sharing

False cache sharing occurs when threads write to different variables that share a cache line (64 bytes).

| Step | Task | Status |
|------|------|--------|
| 4.1 | Audit parallel loops for potential false sharing | Pending |
| 4.2 | Add `#[repr(align(64))]` padding to per-thread accumulators | Pending |
| 4.3 | Use `crossbeam::utils::CachePadded` for thread-local state | Pending |
| 4.4 | Ensure tile processing uses separate output buffers | Pending |
| 4.5 | Benchmark before/after padding changes | Pending |
| 4.6 | Check `rayon` chunk sizes align with cache lines | Pending |

Specific areas to check:
- `estimate_background()`: tile_stats parallel computation
- `matched_filter()`: row-wise convolution with shared output
- `detect_stars()`: union-find parent array updates

### 10.6 Phase 5: Optimize - Memory Allocations

Reduce allocations in hot paths:

| Step | Task | Status |
|------|------|--------|
| 5.1 | Add `#[inline]` to small hot functions | Pending |
| 5.2 | Use `Vec::with_capacity()` everywhere sizes are known | Pending |
| 5.3 | Reuse scratch buffers across iterations | Pending |
| 5.4 | Consider arena allocator for temporary pixel buffers | Pending |
| 5.5 | Profile with DHAT for allocation hotspots | Pending |
| 5.6 | Move allocations outside loops where possible | Pending |
| 5.7 | Use `SmallVec` for small, fixed-size collections | Pending |
| 5.8 | Avoid `collect()` when iterator can be consumed directly | Pending |
| 5.9 | Benchmark before/after allocation optimizations | Pending |

Specific areas to check:
- `sigma_clipped_stats()`: reuses `values` buffer but could avoid clone
- `compute_centroid()`: allocates stamp buffer per star
- `deblend_component()`: multiple Vec allocations in tree building

### 10.7 Phase 6: Algorithm-Level Optimizations

| Step | Task | Status |
|------|------|--------|
| 6.1 | **Background**: Use integral image for fast tile sums | Pending |
| 6.2 | **Convolution**: Cache-oblivious tiled convolution | Pending |
| 6.3 | **Convolution**: Precompute separable kernel weights | Pending |
| 6.4 | **Detection**: Early exit for empty regions | Pending |
| 6.5 | **Union-find**: Path compression + union by rank | Pending |
| 6.6 | **Deblending**: Lazy tree evaluation | Pending |
| 6.7 | **Centroid**: Incremental update for iterative refinement | Pending |
| 6.8 | **L-M fitting**: Better initial parameter estimates | Pending |
| 6.9 | **Median filter**: Huang algorithm for sliding median | Pending |
| 6.10 | **Metrics**: Fused computation of multiple metrics | Pending |

### 10.8 Phase 7: Final Review & Consolidation

| Step | Task | Status |
|------|------|--------|
| 7.1 | Review all algorithms for correctness after refactoring | Pending |
| 7.2 | Run full test suite, ensure no regressions | Pending |
| 7.3 | Run full benchmark suite, document improvements | Pending |
| 7.4 | Identify shared code patterns, extract to common utilities | Pending |
| 7.5 | Look for code duplication between modules | Pending |
| 7.6 | Simplify public API if possible | Pending |
| 7.7 | Add integration tests with real astronomical images | Pending |
| 7.8 | Document performance characteristics in module docs | Pending |
| 7.9 | Create performance regression tests | Pending |
| 7.10 | Final code review for style consistency | Pending |

### 10.9 Shared Code Candidates

Code that appears in multiple places and should be extracted:

| Pattern | Current Locations | Target Location |
|---------|-------------------|-----------------|
| 5x5/6x6 Gaussian elimination | gaussian_fit.rs, moffat_fit.rs | centroid/linear_solver.rs |
| 3x3 kernel convolution | cosmic_ray.rs, median_filter.rs | common/kernel3x3.rs |
| Sigma-clipped statistics | background/mod.rs | common/statistics.rs |
| MAD computation | background/mod.rs, filtering | common/statistics.rs |
| Cache-line padding | Multiple parallel loops | common/cache_aligned.rs |
| SIMD runtime dispatch | Each simd/mod.rs | common/simd_dispatch.rs |

### 10.10 Benchmark Criteria

Each benchmark should test:

1. **Small images** (256×256): Measure overhead, startup cost
2. **Medium images** (2048×2048): Typical use case
3. **Large images** (8192×8192): Memory bandwidth limited
4. **Many stars** (1000+ detections): Scaling with star count
5. **Crowded field**: Deblending performance
6. **Low SNR**: Edge case behavior

Metrics to track:
- Wall-clock time (median of 10 runs)
- CPU cycles (via `perf`)
- Cache misses (L1, L2, L3)
- Branch mispredictions
- Memory bandwidth utilization
- Allocations count and size

### 10.11 Expected Performance Targets

| Module | Current | Target | Improvement |
|--------|---------|--------|-------------|
| Background (4K image) | ~50ms | ~15ms | 3× |
| Convolution (4K image) | ~80ms | ~15ms | 5× |
| Detection (4K image) | ~30ms | ~20ms | 1.5× |
| Centroid (1000 stars) | ~20ms | ~10ms | 2× |
| Median filter (4K) | ~100ms | ~25ms | 4× |
| Full pipeline (4K) | ~400ms | ~100ms | 4× |

---

## 11. Implementation Order Summary

### Iteration 1: Reorganization (No functional changes)
1. Create folder structure
2. Move code to appropriate modules
3. Update imports and public API
4. Ensure all tests pass

### Iteration 2: Testing & Benchmarking Infrastructure
1. Add tests.rs for each module
2. Add bench.rs for each module
3. Create baseline benchmarks

### Iteration 3: SIMD Implementation
1. Add SIMD for highest-impact modules (convolution, background, median)
2. Benchmark each SIMD implementation
3. Add SIMD for medium-impact modules

### Iteration 4: Memory & Cache Optimization
1. Profile for false cache sharing
2. Add cache-line padding
3. Reduce allocations
4. Benchmark improvements

### Iteration 5: Algorithm Optimization
1. Implement algorithmic improvements
2. Benchmark each change
3. Document complexity changes

### Iteration 6: Final Review
1. Code review and cleanup
2. Extract shared utilities
3. Final benchmarks
4. Documentation update
