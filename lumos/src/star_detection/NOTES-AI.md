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
- `median_filter/NOTES-AI.md` - CFA artifact removal, sorting networks, SIMD

## Architecture

### Pipeline Stages (detector/mod.rs -> stages/)
1. **prepare** - RGB to luminance, optional 3x3 median filter for CFA sensors
2. **background** - Tiled sigma-clipped statistics, bilinear interpolation, optional
   iterative refinement with source masking and dilation
3. **fwhm** - Auto-estimation from bright stars (2x sigma first-pass, MAD outlier rejection)
   or manual FWHM, or disabled (no matched filter)
4. **detect** - Optional matched filter convolution (noise-normalized output),
   threshold mask creation, CCL with 8-connectivity (RLE + union-find), deblending,
   size/edge filtering
5. **measure** - Parallel centroid computation (weighted moments + optional Gaussian/Moffat
   L-M fitting), quality metrics (flux, FWHM, eccentricity, SNR, sharpness, roundness,
   L.A.Cosmic Laplacian SNR)
6. **filter** - Cascading quality filters (saturation, SNR, eccentricity, sharpness,
   roundness), MAD-based FWHM outlier removal, spatial-hash duplicate removal

### Pipeline Order Analysis

The pipeline order (background -> filter -> threshold -> label -> deblend -> centroid)
is correct and matches the standard approach used by SExtractor, SEP, and photutils:

1. **Background first**: Must come before thresholding because the threshold is
   `bg + k*sigma`. SExtractor, SEP, photutils all follow this order.
2. **Matched filter before threshold**: The convolution boosts SNR for point sources,
   making fainter stars detectable. SExtractor applies filtering before thresholding.
   DAOFIND also convolves before detecting peaks.
3. **Threshold before labeling**: Standard approach. The threshold mask defines which
   pixels are "detected"; CCL groups them. This is the SExtractor/SEP flow.
4. **Labeling before deblending**: Deblending operates on connected components, so
   components must be identified first. SExtractor's analyse.c operates per-component.
5. **Deblending before centroid**: The deblend step splits blended components into
   individual regions, each of which gets a centroid. Correct order.
6. **Centroid before quality filter**: Quality metrics (SNR, FWHM, eccentricity) are
   computed from centroid data, so centroiding must precede filtering. Correct.

**One deviation from SExtractor**: SExtractor performs detection and measurement in a
single streaming pass through the image (line-by-line). This implementation stores full
intermediate results (background map, label map, region list) which uses more memory
but enables parallel processing and buffer reuse.

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
- **median_filter/** - 3x3 median for CFA artifacts, SIMD sorting network (AVX2/SSE4.1/NEON)
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

## Open Issues

### P2: No Variance Map Convolution for Non-Uniform Noise
- **Location**: convolution/mod.rs
- Output normalization `/ sqrt(sum(K^2))` assumes uniform noise. SExtractor and SEP
  handle per-pixel variance explicitly.
- **Impact**: Low for flat-fielded astrophotography, higher for mosaics.

### P3: Background Mask Fallback Uses All Pixels
- **Location**: background/tile_grid.rs line 249-251
- When too few unmasked pixels remain in a tile during refinement, the fallback
  collects all pixels including masked (star) pixels. This biases the tile's
  background estimate upward in heavily crowded regions.
- **Fix**: Interpolate from neighboring tiles instead of using contaminated pixels.

### ~~P3: Duplicate Import in threshold_mask/mod.rs~~ FIXED
- Duplicate `use rayon::prelude::*;` removed.

## Completed Fixes
1. ~~**Matched filter noise scaling** (P1)~~ **DONE** -- output normalized by sqrt(sum(K^2)).
2. ~~**Dilation before labeling** (P1)~~ **DONE** -- dilation removed from detect stage.
3. ~~**Default 4-connectivity** (P1)~~ **DONE** -- changed to 8-connectivity.
4. ~~**Negative clipping before convolution** (P2)~~ **DONE** -- negative residuals preserved.
5. ~~**Unsafe mutable aliasing in mask_dilation** (P1)~~ **DONE** -- proper `&mut` borrow + SendPtr.
6. ~~**PixelGrid generation counter wrap** (P2)~~ **DONE** -- zero guard added matching NodeGrid.
7. ~~**AtomicUnionFind capacity overflow** (P2)~~ **DONE** -- assert on overflow in make_set().

## Postponed (low impact, revisit only if a real problem arises)
- **L.A.Cosmic fine structure ratio** -- `laplacian_snr` is computed but never used in
  the filter pipeline. Only matters if Laplacian-based filtering is activated.
- **Mode estimator for background** -- Iterative refinement with source masking already
  handles crowded fields. Marginal improvement.
- **Deblend contrast criterion** -- Parent-relative vs root-relative. Only affects deeply
  nested 3+ level blends. Valid alternative.
- **Quality metrics vs DAOFIND** -- Custom definitions work for filtering. Published
  threshold values aren't transferable, but acceptable.
- **Bicubic spline background** -- Negligible for registration with 64px tiles.
- **Gaussian fit rotation angle** -- Adequate for near-circular ground-based PSFs.
- **FWHM discretization correction** -- Only matters for FWHM < 3px.
- **Fit parameters discarded** -- Only position used from profile fits.
- **Voronoi vs flux-weighted deblend** -- Low impact for point-source centroids.
- **SExtractor cleaning pass** -- Quality filter stage partially compensates.
- **Parameter uncertainties from L-M covariance** -- Useful for weighted registration
  but not critical for current centroid-only use case.
- **Separation metric inconsistency** -- Chebyshev in multi-threshold vs Euclidean in
  local maxima. Both err on the side of merging close peaks. Minor inconsistency.

## Cross-Cutting Summary

### What We Do Well vs Industry
- **MAD-based noise** is more robust than SExtractor's clipped std dev (background).
  MAD has 50% breakdown point; std dev has ~0%. Trade-off: MAD has 37% asymptotic
  efficiency, but with 1024 samples/tile, precision is still <5% relative error.
- **SIMD acceleration** throughout -- no other tool (SExtractor, SEP, photutils) has this.
  AVX2/SSE4.1/NEON in: threshold mask, convolution, median filter, profile fitting,
  background interpolation.
- **Iterative background refinement** with source masking matches photutils best practices
- **Mirror boundary** for convolution avoids SExtractor's edge-darkening zero-pad
- **Generation-counter grids** in deblending give O(1) clearing (novel optimization)
- **Early termination** in multi-threshold deblending (SExtractor doesn't have this)
- **Buffer pool** and ArrayVec/SmallVec eliminate heap allocations in hot paths
- **Dual deblending** (local maxima for sparse, multi-threshold for crowded)
- **Moffat PowStrategy** avoids powf in L-M inner loop (HalfInt/Int fast paths)
- **Parallel CCL** with atomic union-find and benchmarked crossover threshold.
  AtomicUnionFind is ABA-safe due to monotonic parent-pointer invariant.
- **Sampling optimization** caps tile statistics at 1024 samples (SExtractor uses all)
- **Fused L-M normal equations** with SIMD -- avoids NxM Jacobian storage, 68-71% faster
- **Bit-packed masks** (BitBuffer2) -- 8x memory reduction, 64-pixel word-level operations
- **Rayon parallelism** in every stage -- SExtractor and SEP are single-threaded

### What SExtractor Does That We Don't (and whether it matters)
- **Weight/variance map input** -- LOW for astrophotography, HIGH for survey data
- **Cleaning pass** -- MEDIUM for crowded fields near bright stars. SExtractor computes
  the contribution to each object's mean surface brightness from its neighbors, subtracts
  it, and accepts the object only if it still exceeds the detection threshold.
- **Bicubic spline background** -- NEGLIGIBLE for registration with 64px tiles
- **Flux-weighted pixel assignment** -- LOW for point-source centroids. SExtractor uses
  Gaussian template weighting; we use Voronoi (nearest peak).
- **Root-relative contrast** -- LOW (parent-relative is a valid alternative). SEP confirmed
  to use root flux: `child_flux >= DEBLEND_MINCONT * root_flux`.
- **CLASS_STAR classifier** -- MEDIUM. Uses 10-input MLP: 8 isophotal areas + peak +
  seeing FWHM. Requires SEEING_FWHM to +-5% for faint sources. Not worth replicating
  for registration use case.
- **Isophotal/Kron photometry** -- NOT NEEDED for registration
- **Weighted least squares** -- MEDIUM. Industry tools (DAOPHOT, SExtractor) use
  inverse-variance weighting `chi2 = sum(((data-model)/sigma_i)^2)`. Our L-M fitting
  is unweighted, which is suboptimal for faint stars with Poisson statistics.
- **Mode estimator** (2.5*med - 1.5*mean) -- LOW. More appropriate for crowded fields
  but iterative refinement partially compensates. See background/NOTES-AI.md.
- **Zero-sum (lowered Gaussian) kernel** -- LOW. DAOFIND's negative-wing kernel
  provides implicit local background subtraction during convolution. Our approach
  of explicit background subtraction before convolution is equally valid.

### What photutils Does That We Don't
- **Watershed segmentation** for deblend pixel assignment (follows intensity gradients)
- **Iterative faintest-peak removal** during deblending (remove weakest, re-watershed)
- **Multiple threshold spacing** modes (exponential, linear, sinh)
- **Pluggable background estimators** (6 location, 3 scale estimators)

### What PixInsight Does That We Don't
- **Wavelet-based (a trous) multiscale structure detection** -- different paradigm from
  matched filtering. Can detect sources at multiple scales simultaneously. PixInsight
  uses dyadic wavelet layers where layer N corresponds to structures of size 2^N pixels.
  This provides natural multi-scale noise suppression (small-scale layers capture noise,
  large-scale layers capture extended objects).
- **Kurtosis-based peak response** (`peakResponse` parameter) for quality filtering
- **Thin plate spline** background (DBE) -- more flexible than tile-based methods
- **Noise estimation via starlet transform** (MRS noise estimator) -- more sophisticated
  than tile-based sigma clipping

### What DAOFIND/DAOStarFinder Does That We Don't
- **Zero-sum kernel**: DAOFIND kernel `K = (G - mean(G))` normalized so output is in
  amplitude units (least-squares Gaussian fit). Implicitly subtracts local background.
- **Noise correction factor**: `relerr = 1/sqrt(sum(K^2) - sum(K)^2/N)` properly
  accounts for the zero-sum normalization. Our `1/sqrt(sum(K^2))` is correct for
  our sum=1 kernel but cannot be directly compared.
- **Formal sharpness/roundness**: DAOFIND computes sharpness as `(peak - mean_4neighbors)
  / peak` and roundness from marginal distributions of the density-enhancement image.
  Our metrics are functionally similar but use different formulas.

### Consistency Issues Across Modules
- ~~Dilation + 4-connectivity~~ **FIXED** -- dilation removed, 8-connectivity default
- ~~Matched filter output not in proper units~~ **FIXED** -- noise-normalized output
- Separation metric: Chebyshev (multi-threshold) vs Euclidean (local maxima) -- minor
- Two different median9 sorting networks (21 vs 25 comparators) -- both correct
- Sharpness and roundness metrics differ from DAOFIND definitions -- custom definitions
  are effective for filtering but published threshold values aren't transferable.
  DAOFIND defaults: sharplo=0.2, sharphi=1.0, roundlo=-1.0, roundhi=1.0.

## Algorithm Correctness Summary

All core algorithms verified against reference implementations:

| Component | Status | Verification |
|-----------|--------|-------------|
| Sigma clipping | Correct | Median center, MAD scale, convergence detection |
| MAD constant | Correct | 1.4826022 = 1/Phi^-1(3/4), f32 precision limit |
| Bilinear interpolation | Correct | Weight clamping, segment-based, SIMD-validated |
| Gaussian Jacobian (6 params) | Correct | All 6 partial derivatives hand-verified |
| Moffat Jacobian (5/6 params) | Correct | All partial derivatives hand-verified |
| L-M damping | Correct | Marquardt multiplicative form H[i][i] *= (1+lambda) |
| FWHM formulas | Correct | Both Gaussian (2.355*sigma) and Moffat (2*alpha*sqrt(2^(1/beta)-1)) |
| Eccentricity | Correct | Standard covariance eigenvalue decomposition |
| SNR formulas | Correct | Three CCD noise models match Howell 2006 |
| Cephes exp() | Correct | <2e-13 relative error, validated in SIMD tests |
| Threshold mask | Correct | Bit-for-bit SIMD vs scalar validation |
| RLE + union-find CCL | Correct | Property-based + ground-truth flood-fill validation |
| AtomicUnionFind | Correct | No ABA problem (monotonic parent invariant) |
| Multi-threshold tree | Correct | Exponential spacing matches SExtractor |
| Sorting network median9 | Correct | Both 21 and 25-comparator networks |
| Matched filter normalization | Correct | `output / sqrt(sum(K^2))` matches SEP approach |
| Mirror boundary convolution | Correct | Preserves flux for sum=1 kernels |
| Separable decomposition | Correct | axis_ratio >= 0.99 dispatches to separable path |

## Missing Features vs SExtractor / PixInsight

### vs SExtractor
- No weight map / variance map input
- No isophotal photometry or Kron/Petrosian radii (not needed for point sources)
- No windowed position parameters (WeightedMoments is approximately equivalent)
- No CLASS_STAR neural network classifier (deblending + quality filters approximate)
- No cleaning pass for spurious detections near bright stars
- No weighted least squares in L-M fitting

### vs PixInsight StarDetection
- No wavelet-based (a trous) multiscale structure detection; uses matched filter instead
- No noise estimation via starlet transform (MRS noise estimator)
- Matched filter is mathematically optimal for known Gaussian PSF but cannot handle
  scale variation across the field
- No adaptive structure size detection (PixInsight auto-tunes minimum structure size)

### vs photutils DAOStarFinder
- ~~Missing noise correction factor~~ **FIXED**
- No negative-wing (zero-sum) kernel for implicit background subtraction
- No formal parameter uncertainties from L-M covariance matrix

### vs All Tools: What We Have That They Don't
- **SIMD acceleration** in every hot path (no other tool has this)
- **Dual deblending modes** selectable via config
- **Moffat PowStrategy** for fast integer/half-integer exponents
- **Generation-counter grids** for O(1) deblend buffer clearing
- **Buffer pool** for zero-allocation across video frame sequences
- **Parallel CCL** with benchmarked sequential/parallel crossover
- **Bit-packed masks** with word-level operations

## References
- Bertin & Arnouts 1996 (SExtractor): A&AS 117, 393
- Stetson 1987 (DAOFIND): PASP 99, 191
- van Dokkum 2001 (L.A.Cosmic): PASP 113, 1420
- Moffat 1969 (Moffat profile): A&A 3, 455
- Barbary 2016 (SEP): JOSS 1(6), 58
- Tukey 1977 (Biweight location): Exploratory Data Analysis
- Howell 2006 (CCD Astronomy): Handbook of CCD Astronomy
- Trujillo et al. 2001 (Moffat PSF): MNRAS 328, 977
- He et al. 2017 (CCL survey): state-of-the-art algorithms review
- Gavin (L-M tutorial): Duke University technical report
- Wu et al. 2009: Optimizing two-pass CCL algorithms, PAA
- Thomas et al. 2006: Comparison of centroid algorithms, MNRAS 371, 323
