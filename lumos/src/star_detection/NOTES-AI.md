# star_detection Module - Implementation Notes

## Overview
Full astronomical source detection pipeline: grayscale conversion, tiled background/noise
estimation with optional iterative refinement, automatic FWHM estimation, matched filter
convolution, SIMD threshold masking, parallel connected component labeling, dual deblending
modes (local maxima + SExtractor multi-threshold tree), sub-pixel centroiding via weighted
moments / Gaussian / Moffat profile fitting, and multi-criteria quality filtering. Designed
for image registration in astrophotography stacking.

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

## Issues vs Industry Standards

### P1: Matched Filter Noise Map Not Scaled After Convolution
- **File**: detector/stages/detect.rs:95-98
- After convolution with a normalized Gaussian kernel (sum=1.0), pixel noise is reduced by
  factor `sqrt(sum(kernel_i^2))`. The threshold mask uses the original unconvolved noise map.
- photutils DAOStarFinder computes `threshold_eff = threshold * kernel.relerr` where
  `relerr = 1/sqrt(sum(K^2) - sum(K)^2/N)` to account for this effect.
- **Impact**: Effective sigma threshold is higher than configured, missing faint stars.
  For FWHM=4px, `sqrt(sum(K^2))` is roughly 0.15, so threshold is ~6.7x too high.
- **Fix**: Scale noise by `sqrt(sum(kernel^2))` after convolution, or multiply sigma
  threshold by the reciprocal.

### P1: Hardcoded Radius-1 Dilation Before Labeling
- **File**: detector/stages/detect.rs:112-118
- Always dilates mask by 1 pixel before CCL, not configurable.
- Merges star pairs within 2 pixels, inflates component areas with background pixels,
  contaminates flux and centroid measurements.
- Neither SExtractor nor DAOFIND perform dilation before labeling.
- 8-connectivity (already supported) handles undersampled PSFs adequately.
- **Fix**: Remove, or gate behind a config flag defaulting to off.

### P2: Background Uses Sigma-Clipped Median, Not SExtractor Mode Estimator
- **File**: background/tile_grid.rs, compute_tile_stats
- SExtractor uses `Mode = 2.5*Median - 1.5*Mean` after sigma clipping, which is ~30% less
  noisy and more robust in crowded fields. Falls back to median when mode/median disagree
  by >30%.
- Current implementation uses sigma-clipped median directly. Adequate for sparse fields but
  suboptimal for crowded regions.
- **Fix**: Implement mode estimator with median fallback.

### P2: Sharpness Differs From DAOFIND Definition
- **File**: centroid/mod.rs:652-656 (compute_metrics)
- Computes `peak / core_flux` (ratio of peak to 3x3 sum of background-subtracted values).
- DAOFIND defines sharpness as `(data_peak - mean_surrounding) / convolved_peak`, using the
  convolved image. photutils IRAFStarFinder uses image moments instead.
- Still works as a cosmic-ray discriminator but published threshold values (0.2-0.7 range)
  are not directly transferable.

### P2: GROUND Roundness Uses Raw Marginal Max, Not Gaussian Fit Height
- **File**: centroid/mod.rs:678-681
- DAOFIND: `2.0 * (Hx - Hy) / (Hx + Hy)` where H = marginal Gaussian fit amplitude.
- Implementation: `(max_x - max_y) / (max_x + max_y)` using raw marginal maxima.
- Missing factor of 2 (range is [-0.5, 0.5] vs DAOFIND [-1, 1]), noisier without fitting.

### P2: SROUND Uses Marginal Asymmetry Instead of Quadrant Symmetry
- **File**: centroid/mod.rs:683-701
- DAOFIND computes SROUND from quadrant sums on the convolved image: ratio of bilateral
  to four-fold symmetry.
- Implementation computes RMS of left/right and top/bottom marginal asymmetry on the
  unconvolved image. Functionally different metric.

### P2: SNR Uses Full Square Stamp Area as npix
- **File**: centroid/mod.rs:646
- Uses `npix = (2*radius+1)^2` (up to 961 for radius=15). Real star flux is concentrated
  in a circular area of ~2-3 FWHM diameter.
- Overestimates noise contribution, producing systematically lower SNR than optimal
  aperture photometry. Not a bug for relative ranking but affects absolute SNR values.
- **Fix**: Use circular aperture area, or sum only pixels above local background.

### P3: Background Interpolation is Bilinear, Not Bicubic Spline
- **File**: background/mod.rs:105-231
- SExtractor uses natural bicubic-spline interpolation between mesh centers.
- Current bilinear interpolation can show gradient discontinuities at tile boundaries.
- Acceptable for most astrophotography use cases; bicubic would improve edge cases.

### P3: Gaussian Fit Model Lacks Rotation Angle (theta)
- **File**: centroid/gaussian_fit/mod.rs
- Model: `A * exp(-0.5*(dx^2/sx^2 + dy^2/sy^2)) + B` with sigma_x, sigma_y but no theta.
- SExtractor fits a rotated elliptical Gaussian with 7 parameters.
- Adequate for ground-based seeing-dominated PSFs (typically near-circular), but cannot
  model field rotation or coma-elongated stars at field edges.

### P3: Filter Stage Rejection Categories Are Mutually Exclusive
- **File**: detector/stages/filter.rs:28-47
- Cascading if-else: each star counted in exactly one rejection category.
- A star failing both SNR and eccentricity only shows in the first matching bucket.
- Misleading for diagnostic tuning. Fix: flag each criterion independently.

### P3: FWHM From Second Moments Lacks Pixel Discretization Correction
- **File**: centroid/mod.rs:619-620
- For undersampled PSFs (FWHM < 3px), second-moment FWHM is biased upward 5-10%.
- Standard correction: `sigma_corrected^2 = sigma_measured^2 - 1/12`.
- Affects FWHM auto-estimation and quality filtering for high-resolution presets.

### P3: Fit Parameters Discarded When Using GaussianFit/MoffatFit
- **File**: centroid/mod.rs:382-412
- Profile fitting provides sigma_x, sigma_y (Gaussian) or alpha, beta (Moffat) for
  more accurate FWHM and eccentricity than second moments.
- Only the position (x0, y0) from the fit is used; metrics computed from second moments.
- **Fix**: Use fit parameters for FWHM and eccentricity when fitting converges.

## Missing Features vs SExtractor / PixInsight

### vs SExtractor
- No weight map / variance map input (SExtractor supports multiple weighting schemes)
- No isophotal photometry or Kron/Petrosian radii (not needed for point sources)
- No windowed position parameters (SExtractor XWIN/YWIN use iterative Gaussian weighting
  similar to WeightedMoments, so this is approximately equivalent)
- No CLASS_STAR neural network classifier (deblending + quality filters serve a similar role)

### vs PixInsight StarDetection
- No wavelet-based (a trous) multiscale structure detection; uses matched filter instead
- No noise estimation via starlet transform (MRS noise estimator)
- No structure map approach for scale-independent detection
- Matched filter approach is mathematically equivalent to PixInsight's correlation method
  for Gaussian PSFs but less robust to scale variation across the field

### vs photutils DAOStarFinder
- Missing the kernel.relerr noise correction factor (see P1 above)
- Sharpness/roundness metrics are non-standard (see P2 issues above)
- Does not support negative-wing kernels (background-subtracting convolution kernel)
  that DAOFIND uses for implicit local background subtraction

## References
- Bertin & Arnouts 1996 (SExtractor): A&AS 117, 393
- Stetson 1987 (DAOFIND): PASP 99, 191
- van Dokkum 2001 (L.A.Cosmic): PASP 113, 1420
- Moffat 1969 (Moffat profile): A&A 3, 455
