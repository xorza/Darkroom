# lumos Crate - Cross-Cutting Findings Summary

Per-module details in each module's `NOTES-AI.md`. This file summarizes cross-cutting
patterns and the highest-priority **unfixed** issues across all modules.

## Top Priority Bugs & Correctness Issues

| Module | Issue | Severity |
|--------|-------|----------|
| stacking | `weighted_mean_indexed()` no guard against `weight_sum == 0` | Low |

## Already Fixed

<details><summary>Click to expand</summary>

- drizzle: Square kernel (polygon clipping) — `sgarea()`/`boxer()` ported from STScI, Jacobian correction
- drizzle: Per-pixel weight maps (`Option<&Buffer2<f32>>`) on `add_image()` and `drizzle_stack()`
- raw: Bayer RCD demosaic implemented (111ms/24MP, 216 MP/s)
- testing: `TestRng::next_f64` fixed to 53-bit precision (was 31-bit)
- testing: `next_gaussian_f32()` added to TestRng, consolidated 8 Box-Muller duplicates
- registration: LO-RANSAC buffer reuse fixed (output parameter + swap, no allocations)
- astro_image: 3-channel FITS loaded as interleaved -> loads planar via `from_planar_channels`
- astro_image: FITS integer data not normalized -> `BitPix::normalization_max()` normalizes to [0,1]
- astro_image: Missing BAYERPAT/FILTER/GAIN/CCD-TEMP -> comprehensive FITS metadata parsing
- astro_image: Float FITS data not normalized -> heuristic normalization (divide by max when max > 2.0)
- math: sigma_clip_iteration index mismatch -> deviations recomputed after quickselect
- math: No compensated summation -> Neumaier scalar + Kahan SIMD hybrid
- star_detection: Matched filter noise not scaled -> normalized by `sqrt(sum(K^2))`
- star_detection: Hardcoded radius-1 dilation -> removed from detect stage
- star_detection: Default 4-connectivity -> changed to 8-connectivity
- star_detection: Unsafe mutable aliasing in mask_dilation -> proper `SendPtr` wrapper
- star_detection: PixelGrid generation counter wrap -> skip-zero guard added
- star_detection: AtomicUnionFind capacity overflow -> assert on overflow
- star_detection: `#[cfg(test)]` helpers in production code -> moved to dedicated test modules
- star_detection: CCL run-merge logic duplicated -> `merge_runs_with_prev()` with `RunMergeUF` trait
- stacking: Linear fit single center -> per-pixel comparison against fitted value
- stacking: Winsorized missing 1.134 correction -> two-phase Huber c=1.5 rewrite
- stacking: Reference frame always frame 0 -> auto-select by lowest MAD
- stacking: Large-stack sorting performance -> adaptive insertion/introsort
- stacking: Cache DefaultHasher non-deterministic -> FNV-1a deterministic hashing
- stacking: Cache missing source file validation -> mtime sidecar files
- stacking: Cache missing madvise -> MADV_SEQUENTIAL on Unix
- stacking: Missing frame-type presets -> bias/dark/flat/light presets
- registration: GammaLut -> replaced with closed-form `gamma_k2()`
- registration: L2/L-inf tolerance mismatch -> search radius * sqrt(2)
- registration: Target point degeneracy -> added `is_sample_degenerate`
- registration: Confidence defaults misaligned -> both use 0.995
- registration: Missing `nearest_one()` -> dedicated method, used in match recovery
- registration: `partial_cmp().unwrap()` -> replaced with `total_cmp()`
- registration: SIP distortion not applied during warping -> `WarpTransform.apply()` applies SIP
- registration: Soft deringing -> PixInsight-style soft clamping (f32 threshold, default 0.3)
- registration: Direct SVD for homography DLT -> Hartley normalization + SVD
- registration: Affine estimator lacked Hartley normalization -> normalize_points() + denormalize
- registration: FMA intrinsics in Lanczos3 SIMD kernel -> no-dering path uses `_mm_fmadd_ps`
- calibration: Single-channel MAD across CFA -> per-CFA-color statistics
- calibration: Sigma floor fails when median=0 -> added absolute floor `1e-4`
- calibration: Per-CFA-channel flat normalization missing -> `divide_by_normalized_cfa()`
- drizzle: Drop size formula inverted (`pixfrac / scale`) -> fixed to `pixfrac * scale`
- drizzle: Square kernel mislabeled -> renamed to Turbo (axis-aligned approximation)
- drizzle: Gaussian kernel FWHM wrong formula -> fixed sigma = `drop_size / 2.3548`
- drizzle: Lanczos used without constraints -> runtime warning when pixfrac!=1 or scale!=1
- drizzle: No output clamping -> Lanczos output clamped to `[0, +inf)` in finalize
- drizzle: min_coverage compared against raw weight -> compared against normalized weight
- drizzle: Interleaved pixel layout -> per-channel `Buffer2<f32>` (no interleaved allocation)
- drizzle: `into_interleaved_pixels()` allocation per frame -> `add_image()` reads channels directly
- drizzle: Finalization single-threaded -> rayon row-parallel normalization and coverage
- drizzle: Kernel implementations duplicated 4x -> extracted `accumulate()` + `add_image_radial()`
- drizzle: add_image_* methods had 8-10 redundant params -> refactored to read dims from AstroImage
- drizzle: Coverage averaged across channels -> uses channel 0 weights only
- astro_image: `save()` cloned entire image -> `to_image(&self)` borrows pixel data
- testing: 16 inline LCG RNG implementations -> `TestRng` struct in `testing/mod.rs`
- testing: DVec2/Star transform functions duplicated 6x -> `Positioned` trait with generic `_impl`
- star_detection: Duplicate test helpers -> import from `common/output/image_writer.rs`
- calibration_masters: Unused imports and unnecessary `.clone().unwrap()` -> cleaned up
- math: `pub mod scalar` in sum exposed detail -> `pub(super) mod scalar`
- astro_image: `CfaImage::demosaic()` unhelpful panic -> descriptive `.expect()` message
- registration: HashSet reallocation in `recover_matches` loop -> pre-allocated with `with_capacity()`
- star_detection: Background mask fallback used contaminated pixels -> uses unmasked pixels first
- star_detection: Bilinear background interpolation (C0) -> natural bicubic spline (C2), matches SEP/SExtractor
- star_detection: Fit params discarded (FWHM/eccentricity from moments) -> fit-derived when available
- star_detection: L.A.Cosmic laplacian_snr computed but unused -> wired into filter stage (`max_laplacian_snr` config)

</details>

## Cross-Cutting Patterns

### 1. Negative Values Not Handled Consistently
- calibration_masters: dark subtraction doesn't clamp to zero (intentional, matches PixInsight)
- stacking: no post-combination clamping
- **Decision**: Preserving negatives in the linear pipeline is correct (prevents positive bias).
  Clamping should only happen at output boundaries (FITS/TIFF export, display).

### 2. Numerical Stability
- All modules verified correct: math sums, registration transforms, stacking formulas,
  calibration operations
- f64 used throughout fitting pipeline (L-M optimizer, Gaussian/Moffat fitting)
- Compensated summation (Kahan SIMD + Neumaier scalar hybrid) in math module
- Minor inconsistency: `weighted_mean_indexed()` in stacking uses naive summation while
  `math::weighted_mean_f32()` uses Neumaier. Negligible for N < 100 but inconsistent.

### 3. SIMD Coverage
- **Well-covered**: math sums, convolution, threshold mask, median filter, profile fitting,
  background interpolation, warp interpolation, X-Trans demosaic, raw normalization (SSE4.1)
- **Gaps**: drizzle accumulation (single-threaded scalar), raw normalization (no AVX2),
  stacking rejection (scalar per-pixel)
- **Dead code**: `sum_and_sum_sq_simd` and `sum_abs_deviations_simd` in background/ (implemented but unused)

### 4. Testing Patterns
- TestRng: centralized deterministic RNG (Knuth MMIX LCG). ~25 call sites.
- `next_gaussian_f32()` on TestRng for Box-Muller Gaussian sampling.
- `next_f64` uses 53-bit precision (full f64 mantissa coverage).
- All SIMD paths have bit-for-bit (or within-epsilon) tests against scalar references.
- Comprehensive property-based and ground-truth tests in star_detection and registration.

### 5. NaN/Inf Handling
- No module explicitly handles NaN/Inf in input data.
- Float FITS data can contain NaN (FITS standard null indicator).
- stacking `median_f32_fast` uses `partial_cmp` with NaN treated as equal (incorrect median if NaN present).
- Expected invariant: source images are NaN-free after loading. Not enforced.
- **Recommendation**: Scan for NaN/Inf in FITS float loader, replace with 0.0.

### 6. Memory Management Patterns
- stacking: in-memory (<75% RAM) or disk-backed (mmap with MADV_SEQUENTIAL)
- star_detection: BufferPool for buffer reuse across video frames
- X-Trans demosaic: DemosaicArena single contiguous 10P allocation with region reuse
- drizzle: per-channel Buffer2<f32> (no interleaving), rayon-parallel finalization
- stacking: per-thread ScratchBuffers via rayon `for_each_init`
- Testing: zero-allocation hot paths via ArrayVec/SmallVec in centroid fitting

## Missing Industry Features (by impact)

| Priority | Feature | Module | Impact | Industry Reference |
|----------|---------|--------|--------|-------------------|
| P1 | FITS writing | astro_image | Primary astro interchange format | All tools |
| P1 | Variance propagation | stacking | No per-pixel noise estimate for downstream use | PixInsight, IRAF |
| ~~P2~~ | ~~True Square kernel (polygon clipping)~~ | ~~drizzle~~ | **DONE** — `DrizzleKernel::Square` | STScI default, Siril default |
| ~~P2~~ | ~~Jacobian correction for non-affine~~ | ~~drizzle~~ | **DONE** — Square kernel uses Jacobian | STScI, Siril, PixInsight |
| P2 | CFA/Bayer drizzle | drizzle | Bypass demosaic artifacts for OSC cameras | Siril, PixInsight, DSS, APP |
| P2 | Rejection maps output | stacking | Per-pixel high/low counts for diagnostics | PixInsight, Siril |
| P2 | Noise-based auto weighting | stacking | w = 1/sigma_bg^2 (optimal MLE) | PixInsight, Siril, APP |
| P2 | Weighted least squares in L-M fitting | star_detection | Unweighted is suboptimal for faint stars | DAOPHOT, SExtractor |
| P2 | RA/DEC metadata | astro_image | Essential for plate solving, mosaic planning | All capture software |
| P2 | Pixel size metadata (XPIXSZ/YPIXSZ) | astro_image | Plate scale calculation | NINA, MaximDL |
| P2 | Parameter uncertainties from L-M | star_detection | Position uncertainty for weighted registration | DAOPHOT |
| P3 | Context/contribution image | drizzle | Per-pixel contributing-frame bitmask | STScI |
| P3 | Variance/error output | drizzle | Propagated variance for photometry | STScI |
| P3 | Parallel drizzle accumulation | drizzle | Row-parallel or per-thread accumulators | PixInsight Fast Drizzle |
| P3 | Additive-only normalization | stacking | Varying pedestal, consistent gain | Siril, PixInsight |
| P3 | Min/Max/Sum combine methods | stacking | Star trails, pixel identification | PixInsight, Siril |
| P3 | Cold pixel detection from flats | calibration | Dead pixels more reliably detected from flats | APP |
| P3 | Generic incremental stepping | registration | Benefit Lanczos2/4/Bicubic (~38% speedup) | Performance optimization |
| Low | Configurable sigma threshold for defect map | calibration | 5.0 hardcoded; PixInsight uses 3.0 | PixInsight, APP |
| Low | NaN/Inf handling in FITS float data | astro_image | Prevents NaN contamination in stacking | Astropy, Siril |
| Low | Multi-HDU FITS support | astro_image | Compressed FITS, observatory formats | cfitsio, Astropy |
| Low | Large-scale rejection (satellite trails) | stacking | Wavelet + growth for coherent structures | PixInsight |

## Module Health Summary

| Module | Bugs Found | Correctness | Key Strength | Key Gap |
|--------|-----------|-------------|--------------|---------|
| math | None | All verified correct | Hybrid Kahan/Neumaier SIMD, SIMD Cephes exp() | None |
| registration | None critical | All 5 estimators + MAGSAC++ verified | Validated by SupeRANSAC 2025 | Generic incremental stepping |
| star_detection | None remaining | Pipeline matches SExtractor | SIMD in every hot path, dual deblending | Weighted least squares |
| stacking | None remaining | All 6 rejection algos verified | MAD-based sigma (more robust than DSS/Siril default) | Rejection maps, auto weighting, variance |
| calibration | None remaining | Formula matches PixInsight/Siril | Per-CFA-color MAD detection, per-CFA flat | Configurable sigma threshold |
| astro_image | None remaining | FITS loading correct | Comprehensive metadata parsing, float normalization | FITS writing, RA/DEC metadata |
| raw | None remaining | X-Trans + Bayer RCD verified | 2.1x faster than libraw (X-Trans), 216 MP/s (Bayer) | None critical |
| drizzle | None remaining | All 5 kernels verified correct | Projective transform, per-pixel weights, polygon clipping, Jacobian | CFA/Bayer drizzle |
| common | None | All correct | CPU feature detection, Buffer2/BitBuffer2 | None |
| testing | None | Deterministic RNG, comprehensive synthetic data | Centralized TestRng, StarFieldBuilder, next_gaussian_f32 | Poisson noise, DetectionMetrics consolidation |

## Recommendations by Priority

### Immediate (data corruption / wrong results)
None remaining.

### Short-term (quality improvements)
1. Add FITS writing support
2. Add RA/DEC and pixel size metadata reading
3. Expose configurable sigma threshold for defect map detection

### Medium-term (feature parity)
4. Add stacking rejection maps (per-pixel high/low counts)
5. Add noise-based auto weighting to stacking (`w = 1/sigma_bg^2`)
6. Add weighted least squares to L-M fitting (inverse-variance weighting)
7. Add parameter uncertainties from L-M covariance matrix
8. Implement CFA/Bayer drizzle
9. Generic incremental stepping for registration interpolation
10. Parallelize drizzle accumulation loops

### Long-term (completeness)
11. Add variance propagation to stacking
12. Add drizzle variance/error propagation
13. Add drizzle context image (per-pixel contributing-frame bitmask)
14. Add stacking Min/Max/Sum combine methods and additive-only normalization
15. Add cold pixel detection from flats in calibration
16. Add NaN/Inf handling in FITS float loader
17. Add multi-HDU FITS support
18. Add large-scale rejection for satellite trails
19. Add missing FITS metadata (DATAMAX, ISOSPEED, CALSTAT, FOCRATIO)

## Verified Correct (no action needed)

These were investigated and confirmed correct against industry references:

- **math**: All summation, statistics, sigma clipping, L-M optimizer, Jacobians (textbook correct)
- **math constants**: MAD_TO_SIGMA (1.4826022), FWHM_TO_SIGMA (2.35482) verified against Astropy/R/GSL
- **math SIMD exp()**: Cephes polynomial < 1e-12 relative error, verified against libm
- **calibration pipeline order**: dark sub -> flat div -> cosmetic correction (matches Siril/PixInsight)
- **calibration negative preservation**: correct (matches PixInsight; prevents positive bias)
- **calibration per-CFA-channel flat normalization**: matches PixInsight "Separate CFA flat scaling"
- **calibration MAD-based detection**: superior to Siril's avgDev (50% vs ~0% breakdown point)
- **raw pipeline order**: black sub -> WB -> demosaic (matches libraw/dcraw/RawTherapee)
- **raw WB normalization**: min=1.0 (matches dcraw convention)
- **raw black level consolidation**: replicates libraw's `adjust_bl()` correctly
- **X-Trans Markesteijn**: MAE ~0.0005 vs libraw, 2.1x speedup, coefficients match reference
- **Bayer RCD demosaic**: 5-step algorithm, 111ms/24MP (216 MP/s), buffer triple-reuse, 11 rayon dispatches
- **registration MAGSAC++**: validated by SupeRANSAC 2025 and Piedade et al. 2025
- **registration SIP direction**: matches Siril v1.3+ convention (forward A/B)
- **registration all 5 transform estimators**: translation, euclidean, similarity, affine, homography
- **registration Lanczos3 + soft deringing**: matches PixInsight algorithm exactly
- **registration k-d tree**: all operations correct, no bugs found
- **drizzle all 4 kernels**: Turbo (axis-aligned drop), Point, Gaussian (FWHM=pixfrac*scale), Lanczos-3
- **drizzle per-pixel weights**: `Option<&Buffer2<f32>>` on add_image/drizzle_stack, 0=exclude, 1=normal
- **drizzle weight accumulation**: two-pass weighted mean, algebraically equivalent to STScI incremental
- **drizzle s^2 factor**: intentionally omitted (correct for surface brightness images)
- **drizzle output**: Lanczos clamped [0,+inf), rayon-parallel finalization, Buffer2 per-channel
- **stacking all 6 rejection algorithms**: sigma clip, winsorized, linear fit, percentile, GESD, none
- **stacking normalization formulas**: global matches Siril "additive with scaling"
- **stacking winsorized**: full two-phase with Huber c=1.5, 1.134 correction, convergence
- **star_detection pipeline order**: matches SExtractor (background -> filter -> threshold -> label -> deblend -> centroid)
- **star_detection background MAD**: 50% breakdown point, superior to SExtractor clipped stddev
- **star_detection background interpolation**: Natural bicubic spline (C2-continuous), matches SEP formula exactly
- **star_detection matched filter**: noise normalization matches SEP approach
- **star_detection CCL**: RLE + atomic union-find, lock-free, ABA-safe
- **star_detection deblending**: multi-threshold tree matches SExtractor algorithm
- **star_detection Gaussian/Moffat Jacobians**: all derivatives hand-verified correct
- **FITS BZERO/BSCALE handling**: cfitsio handles transparently, normalization correct
- **FITS BAYERPAT/ROWORDER**: comprehensive parsing, matches NINA/MaximDL/Siril convention
- **common CPU feature detection**: OnceLock caching correct, NEON compile-time gating correct
- **common Buffer2/BitBuffer2**: bounds checking, padding isolation, alignment all correct
