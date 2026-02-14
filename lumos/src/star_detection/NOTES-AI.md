# star_detection Module - Implementation Notes

## Module Overview

Full astronomical source detection pipeline for image registration in astrophotography
stacking. Six-stage pipeline: grayscale conversion, tiled background/noise estimation with
optional iterative refinement, automatic FWHM estimation, matched-filter convolution, SIMD
threshold masking, parallel connected component labeling, dual deblending modes (local maxima
+ SExtractor multi-threshold tree), sub-pixel centroiding via weighted moments / Gaussian /
Moffat profile fitting, and multi-criteria quality filtering.

See submodule NOTES-AI.md files for detailed per-module analysis:
- `background/NOTES-AI.md` - Tile statistics, interpolation, industry comparison
- `centroid/NOTES-AI.md` - PSF fitting, L-M optimizer, quality metrics
- `convolution/NOTES-AI.md` - Matched filter, noise scaling, SIMD architecture
- `deblend/NOTES-AI.md` - Local maxima + multi-threshold tree algorithms
- `labeling/NOTES-AI.md` - CCL, threshold mask, mask dilation, pipeline integration
- `median_filter/NOTES-AI.md` - CFA artifact removal, sorting networks, SIMD

### Architecture

#### Pipeline Stages (detector/mod.rs -> stages/)
1. **prepare** - RGB to luminance, optional 3x3 median filter for CFA sensors
2. **background** - Tiled sigma-clipped statistics, natural bicubic spline interpolation,
   optional iterative refinement with source masking and dilation
3. **fwhm** - Auto-estimation from bright stars (2x sigma first-pass, MAD outlier rejection)
   or manual FWHM, or disabled (no matched filter)
4. **detect** - Optional matched filter convolution (noise-normalized output),
   threshold mask creation, CCL with 8-connectivity (RLE + union-find), deblending,
   size/edge filtering
5. **measure** - Parallel centroid computation (weighted moments + optional Gaussian/Moffat
   L-M fitting), quality metrics (flux, FWHM, eccentricity, SNR, sharpness, roundness)
6. **filter** - Cascading quality filters (saturation, SNR, eccentricity, sharpness,
   roundness), MAD-based FWHM outlier removal, spatial-hash duplicate removal

#### Key Subsystems
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
  SIMD natural bicubic spline interpolation (C2-continuous), masked pixel collection with
  word-level bit ops
- **threshold_mask/** - SIMD bit-packed mask creation (SSE4.1 / NEON), with and without
  background subtraction variants
- **median_filter/** - 3x3 median for CFA artifacts, SIMD sorting network (AVX2/SSE4.1/NEON)
- **buffer_pool** - BufferPool for f32/u32/bit buffer reuse across frames

#### Configuration
Flat `Config` struct with enums: `Connectivity` (Four/Eight), `CentroidMethod`
(WeightedMoments/GaussianFit/MoffatFit{beta}), `LocalBackgroundMethod`
(GlobalMap/LocalAnnulus), `BackgroundRefinement` (None/Iterative{iterations}), `NoiseModel`.
4 presets: `wide_field()`, `high_resolution()`, `crowded_field()`, `precise_ground()`.
Default: tile_size=64, sigma_threshold=4.0, expected_fwhm=4.0, min_area=5, max_area=500,
min_snr=10.0, 8-connectivity.

---

## Pipeline Analysis (vs SExtractor)

The pipeline order (background -> matched filter -> threshold -> label -> deblend -> centroid
-> quality filter) is correct and matches the standard approach used by SExtractor, SEP, and
photutils.

### Stage-by-Stage Validation

| Stage | This Pipeline | SExtractor | Verdict |
|-------|--------------|------------|---------|
| 1. Background | Tiled sigma-clipped median+MAD, 3x3 tile filter, natural bicubic spline interpolation, optional iterative refinement with source masking | Tiled sigma-clipped mode (2.5*med-1.5*mean), configurable filter, bicubic spline | Correct order. Median vs mode is a valid alternative (more robust, slightly higher bias in crowded fields). Interpolation matches SExtractor/SEP (C2-continuous natural cubic spline). |
| 2. Matched filter | Gaussian convolution, output / sqrt(sum(K^2)) noise normalization | User-supplied filter (default Gaussian .conv), convolved variance map | Correct: filtering before threshold boosts SNR. Noise normalization follows SEP approach. |
| 3. Threshold | Per-pixel local: `pixel > bg + sigma * noise`, bit-packed mask | Same formula: `pixel > bg + DETECT_THRESH * sigma` | Identical approach. Default 4.0 sigma (vs SExtractor 1.5) compensated by matched filter SNR boost. |
| 4. Labeling | RLE + union-find CCL, 8-connectivity, parallel for >65K pixels | Line-by-line streaming, 8-connectivity | Both 8-connectivity. This implementation stores full label map enabling parallelism. |
| 5. Deblending | Two modes: local maxima (Voronoi) + multi-threshold tree | Multi-threshold tree with Gaussian template pixel assignment | Multi-threshold tree matches SExtractor. Local maxima is an additional fast mode. |
| 6. Measurement | Iterative weighted moments -> optional Gaussian/Moffat L-M fitting | XWIN/YWIN windowed centroid, aperture photometry | WeightedMoments ~= XWIN/YWIN. Profile fitting adds higher precision. |
| 7. Filtering | SNR, eccentricity, sharpness, roundness, FWHM outliers, duplicates | CLASS_STAR (MLP), FLAGS (saturation, blending, truncation) | Different metrics but same purpose. No CLASS_STAR equivalent. |

### Key Deviation: Streaming vs Batch

SExtractor performs detection and measurement in a single streaming pass through the image
(line-by-line), minimizing memory usage. This implementation stores full intermediate results
(background map, label map, region list) which uses more memory but enables:
- Rayon parallelism in every stage
- Buffer reuse across video frames via BufferPool
- Separate matched-filter pass (SExtractor does this inline)

### Contrast Criterion Deviation

SExtractor's `DEBLEND_MINCONT` uses root/total component flux as reference:
`child_flux >= DEBLEND_MINCONT * root_flux`. This implementation uses parent-relative
contrast (child flux vs immediate parent). The difference only affects deeply nested
3+ level tree structures. Parent-relative is slightly stricter for nested splits.

---

## Background Estimation Review

### Algorithm Summary

```
estimate_background:
  TileGrid::new_uninit(width, height, tile_size)
  TileGrid::compute(pixels, mask, sigma_clip_iterations)
    fill_tile_stats (parallel per-tile)
      collect pixels (all/sampled/unmasked, MAX_TILE_SAMPLES=1024)
      sigma_clipped_median_mad (math/statistics/mod.rs)
    apply_median_filter (3x3 median on tile grid)
  interpolate_from_grid (parallel per-row, SIMD segments)

refine_background (optional, iterative):
  create_object_mask (threshold + dilation)
  TileGrid::compute(pixels, mask, ...)
  interpolate_from_grid
```

### Comparison with Industry Standards

| Feature | SExtractor | SEP | photutils | This Module |
|---------|-----------|-----|-----------|-------------|
| Tile statistic | Mode: 2.5*med-1.5*mean | Same | Pluggable (6 estimators) | Median only |
| Scale estimator | Clipped std dev | Same | Pluggable (3 estimators) | MAD * 1.4826 |
| Sigma clipping | 3-sigma, iterate to convergence | Same | sigma=3, maxiters=10 | kappa=3, 2-3 iters, early exit |
| Tile filter | Configurable (BACK_FILTERSIZE) | fw, fh (default 3x3) | filter_size param | Fixed 3x3 median |
| Interpolation | Bicubic spline | Bicubic spline | BkgZoomInterpolator (spline) | Natural bicubic spline (C2) |
| Source masking | Automatic via mode estimator | Same | External mask param | Iterative refinement + dilation |
| RMS map | Clipped std dev | Same | Separate RMS estimator | MAD-based sigma per tile |
| SIMD | No | No | No (NumPy) | AVX2, SSE4.1, NEON |
| Parallelism | No | No | NumPy vectorized | Rayon (per-tile, per-row) |

### What We Do Better

1. **MAD-based noise (sigma) estimation**: 50% breakdown point vs ~0% for std dev. More
   robust to source contamination. Trade-off: 37% asymptotic efficiency, but with 1024
   samples/tile, precision is still <5% relative error.
2. **Iterative refinement with source masking + dilation**: More sophisticated than
   SExtractor's mode estimator for handling source contamination. Matches photutils best
   practices.
3. **Sampling optimization**: MAX_TILE_SAMPLES=1024 with 2D strided sampling. SExtractor
   processes all pixels. ~4x faster for 64x64 tiles, ~16x faster for 128x128 tiles.
4. **SIMD-accelerated interpolation**: AVX2/SSE4.1/NEON. No other tool has SIMD in
   background estimation.
5. **Bit-packed mask collection**: Word-level operations skip 64 background pixels at a
   time during masked pixel collection.
6. **Natural bicubic spline interpolation**: Matches SExtractor/SEP C2-continuous surfaces.
   Two-pass algorithm: Y-direction spline precomputed per column, X-direction solved per row.
   SIMD-accelerated cubic polynomial evaluation (AVX2/SSE4.1/NEON).

### What We Do Differently (Trade-offs)

1. **Median vs Mode**: Median is slightly biased upward in crowded fields compared to
   SExtractor's mode estimator. The mode (`2.5*med - 1.5*mean`) is ~30% noisier but less
   affected by source crowding. Iterative refinement with source masking partially
   compensates. Impact: negligible in sparse fields; 1-5% bias in globular cluster cores.

### Known Issues

- **P2: No variance map convolution for non-uniform noise** (convolution/mod.rs).
  `1/sqrt(sum(K^2))` assumes uniform noise. Low impact for flat-fielded astrophotography.
- **P3: Background mask fallback uses all pixels** (tile_grid.rs). When zero unmasked pixels
  remain in a tile, includes masked (star) pixels. Biases estimate upward in very crowded
  tiles. Fix: interpolate from neighbors instead.
- **P3: Fixed 3x3 tile filter** (tile_grid.rs). SExtractor allows configurable filter size
  and differential filtering (BACK_FILTERTHRESH). 3x3 is the standard default.
- **P3: Hardcoded 3.0 sigma clipping** in `compute_tile_stats`. Typically fine; tighter
  clipping (2-sigma) could help in very crowded/nebulous fields.

---

## Source Detection Review

### Matched Filter (convolution/)

Background-subtracted image convolved with Gaussian kernel matching expected PSF to boost
point-source SNR. Output divided by `sqrt(sum(K^2))` for noise normalization (SEP approach).

| Feature | This Implementation | SExtractor | DAOFIND | SEP |
|---------|-------------------|-----------|---------|-----|
| Kernel shape | Gaussian (1D separable or 2D elliptical) | User-supplied (default Gaussian) | Lowered Gaussian (zero-sum, negative wings) | Matched filter template |
| Normalization | Sum = 1.0 | Sum = 1.0 | Zero-sum (implicit bg subtraction) | Full matched filter |
| Edge handling | Mirror/reflect | Zero-pad | Zero-pad | Zero-pad |
| Noise scaling | `output / sqrt(sum(K^2))` | Convolves variance map | `threshold * relerr` | `T = sum(K*D/var) / sqrt(sum(K^2/var))` |
| Separable | Yes (circular), 2D (elliptical) | Always 2D | Always 2D | Depends on implementation |

**Strengths**: Mirror boundary avoids edge-darkening; separable decomposition gives O(n*k)
vs O(n*k^2); SIMD-accelerated (AVX2+FMA, SSE4.1, NEON).

**Correct choices**: Noise normalization matches SEP approach. Negative residuals preserved
(was previously clipped, now fixed). Separable dispatched by axis_ratio threshold (0.99).

### Threshold Masking (threshold_mask/)

Per-pixel local threshold: `pixel > bg + sigma_threshold * max(noise, 1e-6)`. Bit-packed
output (BitBuffer2). SIMD: SSE4.1 (x86_64), NEON (aarch64), scalar fallback. Parallel
per-row via Rayon.

Matches SExtractor formula exactly. Default 4.0 sigma is higher than SExtractor's typical
1.5 sigma, compensated by matched filter SNR boost when FWHM is known.

### Connected Component Labeling (labeling/)

RLE-based with union-find. Two code paths:
- Sequential: for images < 65K pixels
- Parallel: strip-based with atomic union-find + boundary merging for >= 65K pixels

The 65K-pixel threshold was determined empirically by benchmark. Word-level CTZ scanning
extracts runs from bit-packed masks, skipping 64-pixel background blocks.

| Feature | This Implementation | SExtractor | photutils |
|---------|-------------------|-----------|-----------|
| Algorithm | RLE + union-find | Line-by-line streaming | scipy.ndimage.label (pixel-wise) |
| Connectivity | 4 or 8 (default 8) | 8 | 4 or 8 (default 8) |
| Parallelism | Strip-parallel with atomic UF | Single-threaded | Single-threaded (C) |
| Bit-packed input | Yes (word-level scanning) | No (byte/pixel level) | No |

**Strengths**: Lock-free atomic union-find (ABA-safe due to monotonic parent invariant).
RLE + word-level scanning is state-of-the-art (He et al. 2017). Benchmarked crossover.

### Deblending (deblend/)

Two algorithms, selected by `deblend_n_thresholds`:

**Local Maxima** (n_thresholds=0): 8-connected local max detection with prominence and
separation filtering. Voronoi pixel assignment. ArrayVec<_, 8> stack allocation. O(N*P)
where P <= 8 peaks. Best for sparse fields.

**Multi-Threshold Tree** (n_thresholds >= 1): SExtractor-style hierarchical splitting.
Exponential threshold spacing: `thresh[i] = low * (high/low)^(i/n)`. Grid-based BFS with
generation-counter O(1) clearing. Tree pruning by contrast criterion. Early termination
after 4 levels without splits (saves 30-50% iterations). DeblendBuffers for per-thread
reuse via Rayon fold.

| Feature | SExtractor | photutils | This (multi-threshold) | This (local maxima) |
|---------|-----------|-----------|----------------------|-------------------|
| Threshold levels | 32 (DEBLEND_NTHRESH) | 32 (nlevels) | 32 (configurable) | N/A |
| Spacing | Exponential | Exp/linear/sinh | Exponential | N/A |
| Contrast | Root-relative (0.005) | 0.001 | Parent-relative (0.005) | Prominence-based |
| Pixel assignment | Gaussian template weighting | Watershed segmentation | Voronoi | Voronoi |
| Buffer management | Stack-based tree | Python dicts | Generation-counter grids | ArrayVec stack |

**Weakness -- Voronoi pixel assignment**: Both algorithms use nearest-peak distance for
pixel assignment. SExtractor uses Gaussian template weighting; photutils uses watershed
segmentation (follows intensity gradients). Voronoi creates straight boundaries that don't
follow isophotal contours. Impact: moderate for asymmetric blends (>2 mag difference);
adequate for similar-brightness point sources.

---

## Centroiding Analysis

### Three-Tier System

1. **WeightedMoments** (~0.05 px): Iterative Gaussian-weighted first moment with
   `sigma = 0.8 * FWHM / 2.355`, 10 iterations standalone, 2 when fitting follows.
   Nearly identical to SExtractor XWIN/YWIN.

2. **GaussianFit** (~0.01 px): 6-parameter 2D Gaussian L-M fitting
   `[x0, y0, amplitude, sigma_x, sigma_y, background]`. No rotation angle (6 vs
   SExtractor's 7 params). AVX2+FMA with Cephes exp() polynomial (~1e-13 accuracy),
   28 accumulators (21 Hessian + 6 gradient + 1 chi2).

3. **MoffatFit** (~0.01 px): Fixed beta (N=5) or variable beta (N=6). PowStrategy
   for fast integer/half-integer exponents (avoids powf). AVX2+FMA with 21 accumulators.
   Default beta=2.5 (ground-based seeing). Centroid accuracy barely affected by wrong
   beta: <0.15 px even with beta 2.5 vs true 4.0.

### Levenberg-Marquardt Optimizer

Fused normal-equation building: J^T*J, J^T*r, and chi2 computed in single pass, avoiding
NxM Jacobian storage. Marquardt multiplicative damping `H[i][i] *= (1+lambda)`. Three exit
conditions: max_delta < 1e-8, chi2_rel_change < 1e-10, position-only convergence. Lambda
cap at 1e10 prevents infinite loops. Gaussian elimination with partial pivoting for N=5-6.

### Quality Metrics

| Metric | Formula | Notes |
|--------|---------|-------|
| FWHM | Fit-derived (Gaussian sigma / Moffat alpha+beta) when available, moments fallback | Discretization not corrected |
| Eccentricity | Fit-derived `sqrt(1 - sigma_min/sigma_max)` for Gaussian, moments eigenvalues otherwise | Standard definition |
| SNR | Three CCD noise models (Howell 2006) | Uses full square stamp area as npix |
| Sharpness | `peak / core_3x3_flux` | Differs from DAOFIND: `(peak - mean_4neighbors) / peak` |
| Roundness1 | GROUND: from marginal max ratio | Differs from DAOFIND (density-enhancement image) |
| Roundness2 | SROUND: from marginal asymmetry | Custom definition |
| Laplacian SNR | REJECTED | Implementation was flawed: raw Laplacian/noise scales with star brightness, not sharpness. Sharpness filter (`peak/core_flux`) already achieves 100% CR rejection. Removed. |

### Comparison with Industry

| Feature | DAOPHOT | SExtractor | photutils | This Module |
|---------|---------|-----------|-----------|-------------|
| Centroid method | Empirical PSF | XWIN/YWIN | centroid_2dg / fit_2dgaussian | WeightedMoments + Gaussian/Moffat L-M |
| Fitting | Simultaneous multi-star | Per-pixel weighted | scipy.optimize | Custom L-M with SIMD |
| Parameters | Empirical template | 7-param (with rotation) | 7-param | 5-6 param (no rotation) |
| Weighting | Inverse-variance | Inverse-variance | Inverse-variance | Unweighted (flat) |
| Position accuracy | 0.005-0.02 px | 0.02-0.1 px (XWIN) | 0.01-0.05 px (fit) | 0.01-0.05 px (fit), 0.05 px (moments) |
| Heap allocation | Yes | Yes | Yes (Python) | Zero (ArrayVec stack stamps) |
| SIMD | No | No | No | AVX2+FMA, NEON |

### Known Issues

- **P2: No weighted least squares**: Fitting is unweighted (`chi2 = sum((data-model)^2)`).
  Industry tools use inverse-variance weighting. Impact: small for bright stars, significant
  for faint stars near detection threshold.
- **P3: No formal parameter uncertainties**: L-M Hessian available at convergence but not
  inverted for covariance matrix. Would enable per-star position uncertainty for weighted
  registration.
- **P3: Sharpness/roundness differ from DAOFIND**: Custom definitions effective for
  filtering but published DAOFIND thresholds not transferable.
- **P3: No rotation angle in Gaussian fit**: 6 vs 7 params. Adequate for near-circular
  ground-based PSFs.

---

## SIMD Coverage

### Coverage Matrix

| Module | AVX2+FMA | SSE4.1 | NEON | Scalar Fallback |
|--------|----------|--------|------|-----------------|
| threshold_mask | -- | Yes | Yes | Yes |
| convolution (row) | Yes | Yes | Yes | Yes |
| convolution (col) | Yes | Yes | Yes | Yes |
| convolution (2D) | Yes | Yes | Yes | Yes |
| median_filter | Yes | Yes | Yes | Yes |
| background interpolation | Yes | Yes | Yes | Yes |
| background sum/deviation | Yes | Yes | Yes | Yes (dead code) |
| centroid/gaussian_fit | Yes | -- | Yes | Yes |
| centroid/moffat_fit | Yes | -- | Yes | Yes |

### Architecture Notes

- **Runtime dispatch**: `common::cpu_features::has_avx2_fma()` / `has_sse4_1()` with scalar
  fallback. NEON always available on aarch64.
- **Target feature**: `#[target_feature(enable = "avx2,fma")]` on unsafe fns.
- **Validation**: All SIMD paths have bit-for-bit (or within-epsilon) tests against scalar
  reference implementations.
- **Centroid fitting**: f64 arithmetic for numerical stability. AVX2 gives 4x f64
  parallelism, NEON gives 2x.
- **Threshold mask**: Processes 64 pixels per u64 word, in groups of 4 floats (SSE/NEON).
- **Convolution**: 8 pixels/iter (AVX2) or 4 pixels/iter (SSE4.1/NEON) for row pass.
- **Median filter**: 8-wide (AVX2) or 4-wide (SSE4.1/NEON) sorting network.

### Dead SIMD Code

`sum_and_sum_sq_simd` and `sum_abs_deviations_simd` in background/simd/mod.rs are
implemented but unused (`#[allow(dead_code)]`). The actual per-tile sigma clipping uses
scalar code in math/statistics/mod.rs. SIMD acceleration of the sort-based median would be
more impactful than sum/deviation, but the statistics computation is fast enough due to the
MAX_TILE_SAMPLES=1024 cap.

### Performance Impact

| Component | SIMD Speedup | Source |
|-----------|-------------|--------|
| Gaussian L-M (17x17 stamp) | 68-71% faster (25.8us -> 8.3us) | AVX2+Cephes exp |
| Moffat L-M normal equations | ~47% faster | AVX2 fused accumulation |
| Convolution row pass | 4-8x vs scalar | AVX2 FMA |
| Threshold mask | ~4x vs scalar | SSE4.1/NEON (4 pixels/cycle vs 1) |

---

## Industry Comparison

### Comprehensive Feature Matrix

| Feature | SExtractor | SEP | photutils | PixInsight | DAOFIND | This Module |
|---------|-----------|-----|-----------|-----------|---------|-------------|
| **Background** | Mode + spline | Mode + spline | Pluggable + spline | Thin plate spline (DBE) | Local annulus | Median + bicubic spline |
| **Detection** | Matched filter | Matched filter | DAOStarFinder or detect_sources | Wavelet (a trous) | Zero-sum kernel | Matched filter |
| **Labeling** | Streaming 8-conn | Same | scipy.ndimage.label | Internal | Peak-based | RLE + union-find |
| **Deblending** | Multi-threshold tree | Multi-threshold tree | Multi-threshold + watershed | Wavelet layers | N/A | Dual: local maxima + multi-threshold |
| **Centroiding** | XWIN/YWIN | XWIN/YWIN | centroid_2dg | Gaussian/Moffat fit | Marginal centroid | WeightedMoments + Gaussian/Moffat L-M |
| **Noise model** | Clipped std dev | Clipped std dev | MAD/std/biweight | MRS starlet | Empirical | MAD * 1.4826 |
| **Parallelism** | Single-threaded | Single-threaded | NumPy vectorized | Multi-threaded | Single-threaded | Rayon (all stages) |
| **SIMD** | None | None | None | Unknown | None | AVX2/SSE4.1/NEON |
| **Variance maps** | Yes | Yes | Yes | Yes | No | No |
| **Star/galaxy class** | CLASS_STAR (MLP) | No | No | No | No | No |
| **Cleaning** | CLEAN pass | No | No | No | No | No |
| **Photometry** | ISO/AUTO/Kron | ISO/AUTO/Kron | Aperture/PSF | Aperture/PSF | PSF | Stamp flux only |

### What We Do Better Than Industry

1. **SIMD acceleration throughout**: No other source extraction tool has SIMD in hot paths.
   AVX2/SSE4.1/NEON in convolution, threshold mask, median filter, profile fitting,
   background interpolation.
2. **Rayon parallelism in every stage**: SExtractor and SEP are single-threaded. photutils
   relies on NumPy/scipy parallelism.
3. **MAD-based noise estimation**: 50% breakdown point vs ~0% for SExtractor's clipped std
   dev. More robust to source contamination.
4. **Dual deblending modes**: Local maxima for fast sparse-field processing + multi-threshold
   tree for crowded fields. No other tool offers both.
5. **Buffer pool for video frame sequences**: Zero allocation across repeated detection calls.
   BufferPool + DeblendBuffers + ArrayVec/SmallVec eliminate heap allocations in hot paths.
6. **Fused L-M normal equations with SIMD**: Avoids NxM Jacobian storage. 68-71% faster
   than scalar for Gaussian fitting.
7. **Generation-counter grids**: O(1) deblend buffer clearing (novel vs O(n) memset).
8. **Moffat PowStrategy**: Integer/half-integer fast paths avoid expensive powf.
9. **Bit-packed masks**: 8x memory reduction, 64-pixel word-level operations.
10. **Mirror boundary convolution**: Avoids SExtractor's edge-darkening zero-pad.
11. **Early termination in deblending**: 30-50% fewer iterations than SExtractor.
12. **Parallel CCL**: Atomic union-find with empirically benchmarked crossover threshold.

### What Industry Does Better

1. **Variance/weight map support** (SExtractor, SEP, photutils): Per-pixel noise handling
   for mosaics, vignetted fields, bad columns. Our uniform-noise assumption is correct for
   flat-fielded astrophotography but insufficient for survey data.
2. **Weighted least squares in fitting** (DAOPHOT, SExtractor): Inverse-variance weighting
   `chi2 = sum(((data-model)/sigma_i)^2)`. Improves accuracy for faint stars.
3. **Gaussian template pixel assignment** (SExtractor) / **Watershed** (photutils): Better
   than our Voronoi for asymmetric blends.
4. **Wavelet multiscale detection** (PixInsight): Can detect sources at multiple scales
   simultaneously. Matched filter is optimal for known PSF but cannot handle scale variation
   across the field.
5. **Cleaning pass** (SExtractor): Removes spurious detections near bright star wings.
   Quality filter stage partially compensates.
6. **Pluggable background estimators** (photutils): 6 location + 3 scale estimators vs
   our fixed median+MAD.
7. **Mode estimator** (SExtractor): `2.5*med - 1.5*mean` is less biased than median in
   crowded fields, at cost of 30% more noise.
8. **CLASS_STAR classifier** (SExtractor): 10-input MLP for star/galaxy separation.
   Not needed for registration use case.
9. **Root-relative contrast** (SExtractor/SEP): Standard approach vs our parent-relative.
   Difference only affects deeply nested 3+ level splits.

---

## Missing Features

### HIGH Priority (would improve accuracy in common scenarios)

1. **Weighted least squares in L-M fitting** (centroid/): Industry tools use inverse-variance
   weighting. Unweighted fitting is suboptimal for faint stars with Poisson statistics.
   Location: `lm_optimizer.rs`, `gaussian_fit/mod.rs`, `moffat_fit/mod.rs`.

### MEDIUM Priority (important for specific use cases)

2. **Variance/weight map input**: Per-pixel noise handling for mosaics, vignetted fields,
   images with bad columns. Requires changes in: convolution (convolve variance map),
   threshold mask (per-pixel noise), background (weighted statistics).
3. **SExtractor cleaning pass**: Remove spurious detections near bright star wings. Computes
   contribution to each object's mean surface brightness from neighbors, subtracts it,
   accepts only if still above threshold. Quality filter stage partially compensates.
4. **Parameter uncertainties from L-M covariance**: Invert Hessian at convergence for 1-sigma
   position uncertainties. Enables weighted registration (`weight = 1/sigma_pos^2`).
   Trivial computation (~1us for 6x6 matrix) since Hessian already available.

### LOW Priority (marginal improvement for registration use case)

5. **Mode/biweight location estimator** for crowded field backgrounds. Iterative refinement
   already compensates. SExtractor mode: `2.5*med - 1.5*mean`. Biweight location (Tukey
   1977) is the most statistically robust option (98.2% efficiency vs median's 64%).
6. **Gaussian fit rotation angle**: 7th parameter (theta). Adequate for near-circular
   ground-based PSFs; needed for optical systems with astigmatism.
7. **Watershed/flux-weighted pixel assignment**: Better than Voronoi for asymmetric blends.
   Simple improvement: assign by `peak_flux * exp(-dist^2/(2*sigma^2))` instead of
   `min(dist)`.
8. **DAOFIND-style zero-sum kernel**: Implicit local background subtraction during
   convolution. Our explicit background subtraction is equally valid.
9. **Formal DAOFIND sharpness/roundness**: Current custom metrics work for filtering but
   published DAOFIND thresholds aren't transferable.

### NOT NEEDED for registration

- Isophotal/Kron/Petrosian photometry
- CLASS_STAR neural network classifier
- Windowed position parameters (WeightedMoments ~= XWIN)
- Wavelet multiscale detection (matched filter is optimal for known PSF)
- Multi-band deblending (scarlet/scarlet2)

---

## Recommendations

### Short-term (high impact, low effort)

1. **Add parameter uncertainties**: Invert J^T*J at L-M convergence. Trivial computation
   for N=5-6. Enables weighted star matching in registration.

### Medium-term (moderate impact, moderate effort)

2. **Implement weighted least squares**: Add per-pixel variance weighting to L-M models.
   Requires `gain` and `read_noise` parameters. Main change in `batch_build_normal_equations`
   methods.

3. **Implement SExtractor mode estimator as option**: Simple formula
   `mode = 2.5*med - 1.5*mean` after sigma clipping, with fallback to median when they
   disagree by >30%. ~5 lines in `compute_tile_stats`. Add to Config as `TileStatistic`
   enum.

4. **Intensity-weighted pixel assignment in deblending**: Replace Voronoi
   (`min(dist)`) with `max(peak_flux * exp(-dist^2 / (2*sigma^2)))`. Small change in
   `local_maxima/mod.rs:find_nearest_peak` and `multi_threshold/mod.rs:assign_pixels_to_objects`.

### Long-term (significant effort, diminishing returns for registration)

5. **Variance map support**: Pervasive change across convolution, threshold, background.
6. **SExtractor cleaning pass**: New pipeline stage between measure and filter.

---

## Algorithm Correctness Summary

All core algorithms verified against reference implementations:

| Component | Status | Verification |
|-----------|--------|-------------|
| Sigma clipping | Correct | Median center, MAD scale, convergence detection |
| MAD constant | Correct | 1.4826022 = 1/Phi^-1(3/4), f32 precision limit |
| Natural bicubic spline interpolation | Correct | C2-continuous, matches SEP formula, SIMD-validated, tridiagonal solver verified |
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

## Consistency Issues Across Modules

- Separation metric: Chebyshev (multi-threshold) vs Euclidean (local maxima) -- minor,
  both err on the side of merging close peaks.
- Two different median9 sorting networks (21 vs 25 comparators) -- both correct.
- Sharpness and roundness metrics differ from DAOFIND definitions -- custom definitions
  are effective for filtering but published threshold values aren't transferable.

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
- Melchior et al. 2018 (scarlet): Astronomy and Computing 24, 129
- Lupton et al. (SDSS deblender): Princeton deblender documentation
