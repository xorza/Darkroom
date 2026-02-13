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

### ~~P1 (Safety): Unsafe Mutable Aliasing in mask_dilation~~ — FIXED
- Raw pointer now obtained from `words_mut().as_mut_ptr()` (proper `&mut` borrow)
  before entering parallel regions. Wrapped in `SendPtr` for thread safety.

### P2: PixelGrid Generation Counter Wrap-to-Zero Not Guarded
- **Location**: deblend/multi_threshold/mod.rs line 104
- `wrapping_add(1)` can wrap to 0 (the sentinel value), causing phantom valid cells.
- `NodeGrid` (line 234) correctly guards against this; `PixelGrid` does not.
- **Trigger**: After ~4 billion resets. Unlikely but possible in long video sessions.
- **Fix**: Add `if self.current_generation == 0 { self.current_generation = 1; }`

### P2: AtomicUnionFind Capacity Overflow Silently Ignored
- **Location**: labeling/mod.rs line 731-735
- If label count exceeds pre-allocated capacity, label is returned without being stored.
- Subsequent `find()` on this label would access uninitialized memory.
- **Fix**: Assert or panic on overflow.

### P2: No Variance Map Convolution for Non-Uniform Noise
- **Location**: convolution/mod.rs
- Output normalization `/ sqrt(sum(K^2))` assumes uniform noise. SExtractor and SEP
  handle per-pixel variance explicitly.
- **Impact**: Low for flat-fielded astrophotography, higher for mosaics.

## Completed Fixes
1. ~~**Matched filter noise scaling** (P1)~~ **DONE** — output normalized by sqrt(sum(K^2)).
2. ~~**Dilation before labeling** (P1)~~ **DONE** — dilation removed from detect stage.
3. ~~**Default 4-connectivity** (P1)~~ **DONE** — changed to 8-connectivity.
4. ~~**Negative clipping before convolution** (P2)~~ **DONE** — negative residuals preserved.

## Postponed (low impact, revisit only if a real problem arises)
- **L.A.Cosmic fine structure ratio** — `laplacian_snr` is computed but never used in
  the filter pipeline. Only matters if Laplacian-based filtering is activated.
- **Mode estimator for background** — Iterative refinement with source masking already
  handles crowded fields. Marginal improvement.
- **Deblend contrast criterion** — Parent-relative vs root-relative. Only affects deeply
  nested 3+ level blends. Valid alternative.
- **Quality metrics vs DAOFIND** — Custom definitions work for filtering. Published
  threshold values aren't transferable, but acceptable.
- **Bicubic spline background** — Negligible for registration with 64px tiles.
- **Gaussian fit rotation angle** — Adequate for near-circular ground-based PSFs.
- **FWHM discretization correction** — Only matters for FWHM < 3px.
- **Fit parameters discarded** — Only position used from profile fits.
- **Voronoi vs flux-weighted deblend** — Low impact for point-source centroids.
- **SExtractor cleaning pass** — Quality filter stage partially compensates.
- **Parameter uncertainties from L-M covariance** — Useful for weighted registration
  but not critical for current centroid-only use case.
- **Separation metric inconsistency** — Chebyshev in multi-threshold vs Euclidean in
  local maxima. Both err on the side of merging close peaks. Minor inconsistency.

## Cross-Cutting Summary

### What We Do Well vs Industry
- **MAD-based noise** is more robust than SExtractor's clipped std dev (background)
- **SIMD acceleration** throughout — no other tool (SExtractor, SEP, photutils) has this
- **Iterative background refinement** with source masking matches photutils best practices
- **Mirror boundary** for convolution avoids SExtractor's edge-darkening zero-pad
- **Generation-counter grids** in deblending give O(1) clearing (novel optimization)
- **Early termination** in multi-threshold deblending (SExtractor doesn't have this)
- **Buffer pool** and ArrayVec/SmallVec eliminate heap allocations in hot paths
- **Dual deblending** (local maxima for sparse, multi-threshold for crowded)
- **Moffat PowStrategy** avoids powf in L-M inner loop (HalfInt/Int fast paths)
- **Parallel CCL** with atomic union-find and benchmarked crossover threshold
- **Sampling optimization** caps tile statistics at 1024 samples (SExtractor uses all)
- **Fused L-M normal equations** with SIMD — avoids NxM Jacobian storage, 68-71% faster

### What SExtractor Does That We Don't (and whether it matters)
- **Weight/variance map input** — LOW for astrophotography, HIGH for survey data
- **Cleaning pass** — MEDIUM for crowded fields near bright stars
- **Bicubic spline background** — NEGLIGIBLE for registration with 64px tiles
- **Flux-weighted pixel assignment** — LOW for point-source centroids
- **Root-relative contrast** — LOW (parent-relative is a valid alternative)
- **CLASS_STAR classifier** — MEDIUM (deblending + filters approximate this)
- **Isophotal/Kron photometry** — NOT NEEDED for registration

### Consistency Issues Across Modules
- ~~Dilation + 4-connectivity~~ **FIXED** — dilation removed, 8-connectivity default
- ~~Matched filter output not in proper units~~ **FIXED** — noise-normalized output
- Separation metric: Chebyshev (multi-threshold) vs Euclidean (local maxima) — minor
- Two different median9 sorting networks (21 vs 25 comparators) — both correct

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
- ~~Missing noise correction factor~~ **FIXED**
- No negative-wing (zero-sum) kernel for implicit background subtraction

## References
- Bertin & Arnouts 1996 (SExtractor): A&AS 117, 393
- Stetson 1987 (DAOFIND): PASP 99, 191
- van Dokkum 2001 (L.A.Cosmic): PASP 113, 1420
- Moffat 1969 (Moffat profile): A&A 3, 455
- Barbary 2016 (SEP): JOSS 1(6), 58
- Tukey 1977 (Biweight location): Exploratory Data Analysis
