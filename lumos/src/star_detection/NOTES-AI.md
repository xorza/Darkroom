# star_detection Module - Implementation Notes

## Overview
Full astronomical source detection pipeline: grayscale conversion, tiled background/noise
estimation with optional iterative refinement, automatic FWHM estimation, matched filter
convolution, SIMD threshold masking, parallel connected component labeling, dual deblending
modes (local maxima + SExtractor multi-threshold tree), sub-pixel centroiding via weighted
moments / Gaussian / Moffat profile fitting, and multi-criteria quality filtering. Designed
for image registration in astrophotography stacking.

See submodule NOTES-AI.md files for detailed per-module analysis:
- `background/NOTES-AI.md` - Tile statistics, interpolation, industry comparison
- `centroid/NOTES-AI.md` - PSF fitting, L-M optimizer, quality metrics
- `convolution/NOTES-AI.md` - Matched filter, noise scaling, SIMD architecture
- `deblend/NOTES-AI.md` - Local maxima + multi-threshold tree algorithms
- `labeling/NOTES-AI.md` - CCL, threshold mask, mask dilation, pipeline integration

## Architecture

### Pipeline Stages (detector/mod.rs -> stages/)
1. **prepare** - RGB to luminance, optional 3x3 median filter for CFA sensors
2. **background** - Tiled sigma-clipped statistics, bilinear interpolation, optional
   iterative refinement with source masking and dilation
3. **fwhm** - Auto-estimation from bright stars (2x sigma first-pass, MAD outlier rejection)
   or manual FWHM, or disabled (no matched filter)
4. **detect** - Optional matched filter convolution, threshold mask creation, radius-1
   dilation, CCL (RLE + union-find), deblending, size/edge filtering
5. **measure** - Parallel centroid computation (weighted moments + optional Gaussian/Moffat
   L-M fitting), quality metrics (flux, FWHM, eccentricity, SNR, sharpness, roundness,
   L.A.Cosmic Laplacian SNR)
6. **filter** - Cascading quality filters (saturation, SNR, eccentricity, sharpness,
   roundness), MAD-based FWHM outlier removal, spatial-hash duplicate removal

### Key Subsystems
- **convolution/** - Separable Gaussian O(n*k), elliptical 2D O(n*k^2), SIMD row/col passes
- **centroid/** - Weighted moments, Gaussian 6-param L-M, Moffat 5/6-param L-M with
  PowStrategy (integer/half-integer beta optimization), SIMD normal equation building
  (AVX2+FMA / NEON), Gaussian elimination linear solver
- **labeling/** - Sequential and parallel RLE-based CCL, word-level bit scanning, atomic
  union-find with boundary merging, 65K-pixel parallel threshold
- **deblend/** - Local maxima (8-neighbor, prominence + separation filters, Voronoi
  partitioning) and multi-threshold tree (exponential spacing, BFS on PixelGrid with
  generation-counter O(1) clearing, contrast criterion)
- **background/** - TileGrid with sigma-clipped median+MAD, 3x3 median filter on tiles,
  SIMD bilinear interpolation, masked pixel collection with word-level bit ops
- **threshold_mask/** - SIMD bit-packed mask creation (SSE4.1 / NEON), with and without
  background subtraction variants
- **buffer_pool** - BufferPool for f32/u32/bit buffer reuse across frames

## Strengths
- Pipeline mirrors SExtractor (Bertin & Arnouts 1996) with DAOFIND-style metrics
- Both SExtractor multi-threshold and DAOFIND local maxima deblending available
- BufferPool eliminates repeated heap allocations across video frames
- DeblendBuffers with generation-counter gives O(1) reset vs O(n) clearing
- SIMD throughout hot paths: threshold mask, convolution, median filter, profile fitting
- Correct Levenberg-Marquardt with fused normal-equation building avoids Jacobian storage
- Moffat PowStrategy: integer exponents use repeated multiplication, half-integers use
  sqrt, avoiding expensive powf in the L-M inner loop
- Parallel CCL benchmarked: 65K-pixel threshold for sequential-to-parallel crossover
- L.A.Cosmic Laplacian SNR metric (van Dokkum 2001) for cosmic ray rejection
- 4 preset configs: wide_field, high_resolution, crowded_field, precise_ground
- Comprehensive test coverage with synthetic stars and real astronomical data
- MAD-based noise estimation is more robust than SExtractor's clipped std dev
- Mirror boundary for convolution is superior to SExtractor's zero-pad for sum=1 kernels
- Early termination in multi-threshold deblending saves 30-50% iterations

## Issues vs Industry Standards

### P1: Matched Filter Noise Not Scaled After Convolution -- CRITICAL
- **File**: detector/stages/detect.rs:95-98, convolution/mod.rs:73
- After convolution with normalized Gaussian kernel (sum=1.0), noise is reduced by
  `sqrt(sum(K^2))`. Threshold mask uses the **original** unconvolved noise map.
- For FWHM=4px, `sqrt(sum(K^2))` ~ 0.117. Effective threshold is **~8.5x** too high.
  A configured 3.0 sigma behaves like **25.5 sigma**. Only very bright stars detected.
- SEP matched filter: `SNR = conv(D,K) / (sigma * sqrt(sum(K^2)))` -- output in SNR units.
- DAOFIND: `threshold_eff = threshold * relerr` where `relerr = 1/sqrt(sum(K^2))`.
- **Fix**: Divide convolved output by `sqrt(sum(K_1d^2)^2)` (SEP approach). See
  convolution/NOTES-AI.md for implementation details.

### P1: Hardcoded Radius-1 Dilation Before Labeling
- **File**: detector/stages/detect.rs:112-118
- Always dilates mask by 1 pixel before CCL, not configurable.
- **No standard tool** (SExtractor, DAOFIND, photutils, SEP) performs dilation before
  labeling. This is unique to this implementation.
- Merges star pairs within 2 pixels, inflates component areas, contaminates centroids.
- **Fix**: Remove entirely. Use 8-connectivity instead (see next issue).

### P1: Default 4-Connectivity Is Non-Standard
- **File**: config.rs -- default connectivity is 4-connected.
- SExtractor, photutils, and SEP all default to **8-connectivity**.
- 4-connectivity fragments above-threshold footprints that touch only diagonally.
- The config comment claiming "matches SExtractor" is **incorrect**.
- **Fix**: Change default to 8-connectivity. This eliminates the need for dilation.
- **Note**: The dilation was likely a workaround for 4-connectivity missing diagonals.

### P2: L.A.Cosmic Missing Fine Structure Ratio -- HIGH IMPACT
- **File**: centroid/mod.rs (Laplacian SNR computation)
- van Dokkum (2001) L.A.Cosmic uses TWO criteria: (1) Laplacian SNR AND (2) fine
  structure ratio = Laplacian / median-filtered-Laplacian > threshold.
- Without the fine structure ratio, bright star peaks (large Laplacian from sharpness)
  get flagged alongside actual cosmic rays.
- The fine structure ratio discriminates: CRs have high Laplacian but low surrounding
  signal (high ratio), while stars have proportional surrounding signal (low ratio).
- **Fix**: Compute median-filtered Laplacian, divide. Threshold at ~2.0 per paper.

### P2: Negative Clipping Before Convolution
- **File**: convolution/mod.rs:73 -- `(px - bg).max(0.0)`
- Neither SExtractor, DAOFIND, nor SEP clip negatives before convolution.
- Biases convolved image upward by ~0.798*sigma. Once P1 noise scaling is fixed, this
  bias becomes proportionally larger.
- **Fix**: Remove `.max(0.0)`.

### P2: Background Uses Sigma-Clipped Median, Not Mode/Biweight
- **File**: background/tile_grid.rs, compute_tile_stats
- SExtractor: `Mode = 2.5*Median - 1.5*Mean` (~30% less noisy in crowded fields).
  photutils recommends biweight location (98.2% efficiency vs median's 64%).
- Current sigma-clipped median is adequate for sparse fields. Iterative refinement
  with source masking partially compensates.
- **Fix**: Implement SExtractor mode formula with median fallback (simplest).
  See background/NOTES-AI.md for biweight location details.

### P2: Deblend Contrast Criterion Differs from SExtractor
- **File**: deblend/multi_threshold/mod.rs L787-826
- This implementation uses **parent-relative** contrast criterion.
- SExtractor uses **root/total component** relative contrast (confirmed from source).
- Parent-relative is stricter for nested splits. Difference mainly affects deeply
  nested tree structures with 3+ levels.
- See deblend/NOTES-AI.md for details.

### P2: Quality Metrics Differ from DAOFIND Definitions
- **Sharpness** (centroid/mod.rs:652-656): `peak/core_flux` vs DAOFIND
  `(peak - mean_surrounding) / convolved_peak`. Different metric.
- **GROUND roundness** (centroid/mod.rs:678-681): raw marginal max vs Gaussian fit
  amplitude. Missing factor of 2 (range [-0.5,0.5] vs DAOFIND [-1,1]).
- **SROUND** (centroid/mod.rs:683-701): marginal asymmetry vs quadrant symmetry.
- **SNR** (centroid/mod.rs:646): full square stamp area `(2r+1)^2` vs circular aperture.
  Underestimates SNR by factor of sqrt(pi/4) to sqrt(2).
- Still functional for quality filtering but published threshold values not transferable.

### P3: Background Interpolation is Bilinear, Not Bicubic Spline
- **File**: background/mod.rs:105-231
- SExtractor uses bicubic spline. Negligible difference for registration (interpolation
  errors far below noise floor with 64px tiles).

### P3: Gaussian Fit Model Lacks Rotation Angle (theta)
- **File**: centroid/gaussian_fit/mod.rs
- 6 params vs SExtractor's 7 (with theta). Adequate for near-circular ground-based PSFs.

### P3: Filter Rejection Categories Are Mutually Exclusive
- **File**: detector/stages/filter.rs:28-47
- Cascading if-else: first failure reported. SExtractor uses bit-field FLAGS for all.

### P3: FWHM Lacks Pixel Discretization Correction
- **File**: centroid/mod.rs:619-620
- `sigma^2 - 1/12` correction missing. Only matters for FWHM < 3px.

### P3: Fit Parameters Discarded
- **File**: centroid/mod.rs:382-412
- Profile fit gives sigma_x, sigma_y (Gaussian) or alpha, beta (Moffat) for better
  FWHM/eccentricity than second moments. Only position used.

### P3: Deblend Pixel Assignment Uses Voronoi, Not Flux-Weighted
- Both algorithms assign pixels by nearest-peak Euclidean distance.
- SExtractor: probabilistic elliptical Gaussian weighting (Mahalanobis distance).
- photutils: watershed segmentation (follows intensity gradients).
- Impact: moderate for high dynamic range blends (>2 mag difference).

### P3: Missing SExtractor Cleaning Pass
- SExtractor removes spurious detections near bright star wings (CLEAN parameter).
- Quality filter stage partially compensates but not equivalent.

## Cross-Cutting Summary

### Most Impactful Fixes (ordered by expected improvement)
1. **Fix noise scaling** (P1) -- currently only detecting very bright stars. Single
   scalar division on convolved output. Expected: 5-10x more stars detected.
2. **Remove dilation + fix connectivity** (P1) -- remove radius-1 dilation, change
   default to 8-connectivity. Prevents merging close pairs, cleaner components.
3. **Add L.A.Cosmic fine structure ratio** (P2) -- prevents misclassifying bright star
   peaks as cosmic rays. Compute median-filtered Laplacian, divide.
4. **Remove negative clipping** (P2) -- remove `.max(0.0)` before convolution.
5. **Mode estimator for background** (P2) -- SExtractor mode formula, ~5 lines of code.

### What We Do Well vs Industry
- **MAD-based noise** is more robust than SExtractor's clipped std dev (background)
- **SIMD acceleration** throughout -- no other tool (SExtractor, SEP, photutils) has this
- **Iterative background refinement** with source masking matches photutils best practices
- **Mirror boundary** for convolution avoids SExtractor's edge-darkening zero-pad
- **Generation-counter grids** in deblending give O(1) clearing (novel optimization)
- **Early termination** in multi-threshold deblending (SExtractor doesn't have this)
- **Buffer pool** and ArrayVec/SmallVec eliminate heap allocations in hot paths
- **Dual deblending** (local maxima for sparse, multi-threshold for crowded)
- **Moffat PowStrategy** avoids powf in L-M inner loop (HalfInt/Int fast paths)
- **Parallel CCL** with atomic union-find and benchmarked crossover threshold

### What SExtractor Does That We Don't (and whether it matters)
- **Weight/variance map input** -- LOW for astrophotography, HIGH for survey data
- **Cleaning pass** -- MEDIUM for crowded fields near bright stars
- **Bicubic spline background** -- NEGLIGIBLE for registration with 64px tiles
- **Flux-weighted pixel assignment** -- LOW for point-source centroids
- **Root-relative contrast** -- LOW (parent-relative is a valid alternative)
- **CLASS_STAR classifier** -- MEDIUM (deblending + filters approximate this)
- **Isophotal/Kron photometry** -- NOT NEEDED for registration

### Consistency Issues Across Modules
- Dilation + 4-connectivity is an inconsistent workaround; fix is 8-connectivity alone
- Matched filter output not in proper units, affecting all downstream thresholding
- Quality metrics (sharpness, roundness) use custom definitions but config defaults
  and documentation reference DAOFIND ranges, which don't apply
- L.A.Cosmic implementation is incomplete (missing fine structure ratio)

## Missing Features vs SExtractor / PixInsight

### vs SExtractor
- No weight map / variance map input
- No isophotal photometry or Kron/Petrosian radii (not needed for point sources)
- No windowed position parameters (WeightedMoments is approximately equivalent)
- No CLASS_STAR neural network classifier (deblending + quality filters approximate)
- No cleaning pass for spurious detections near bright stars

### vs PixInsight StarDetection
- No wavelet-based (a trous) multiscale structure detection; uses matched filter instead
- No noise estimation via starlet transform (MRS noise estimator)
- Matched filter is mathematically optimal for known Gaussian PSF but cannot handle
  scale variation across the field

### vs photutils DAOStarFinder
- Missing noise correction factor (P1)
- Sharpness/roundness metrics non-standard (P2)
- No negative-wing (zero-sum) kernel for implicit background subtraction

## References
- Bertin & Arnouts 1996 (SExtractor): A&AS 117, 393
- Stetson 1987 (DAOFIND): PASP 99, 191
- van Dokkum 2001 (L.A.Cosmic): PASP 113, 1420
- Moffat 1969 (Moffat profile): A&A 3, 455
- Barbary 2016 (SEP): JOSS 1(6), 58
- Tukey 1977 (Biweight location): Exploratory Data Analysis
