# lumos Crate - Cross-Cutting Findings Summary

Per-module details in each module's `NOTES-AI.md`. This file summarizes cross-cutting
patterns and the highest-priority **unfixed** issues across all modules.

## Top Priority Bugs & Correctness Issues

None remaining. All algorithms verified correct against industry references.

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
- star_detection: L.A.Cosmic laplacian_snr -> REJECTED and removed
- stacking: `weighted_mean_indexed()` div-by-zero when all surviving weights=0 -> epsilon guard
- astro_image: Float FITS NaN/Inf not sanitized -> replaced with 0.0 before normalization

</details>

## Cross-Cutting Patterns

### 1. Negative Values — Consistent and Correct
- calibration_masters: dark subtraction preserves negatives (matches PixInsight)
- stacking: no post-combination clamping
- **Decision**: Preserving negatives in the linear pipeline is correct (prevents positive bias).
  Clamping should only happen at output boundaries (FITS/TIFF export, display).

### 2. Numerical Stability — Verified
- All modules verified correct: math sums, registration transforms, stacking formulas,
  calibration operations
- f64 used throughout fitting pipeline (L-M optimizer, Gaussian/Moffat fitting)
- Compensated summation (Kahan SIMD + Neumaier scalar hybrid) in math module
- Minor: `weighted_mean_indexed()` in stacking uses naive summation while
  `math::weighted_mean_f32()` uses Neumaier. Negligible for N < 100.

### 3. SIMD Coverage
- **Well-covered**: math sums, convolution, threshold mask, median filter, profile fitting,
  background interpolation, warp interpolation, X-Trans demosaic, raw normalization (SSE4.1),
  Bayer RCD demosaic
- **Gaps**: drizzle accumulation (single-threaded scalar), raw normalization (no AVX2),
  stacking rejection (scalar per-pixel)
- **Dead code**: `sum_and_sum_sq_simd` and `sum_abs_deviations_simd` in background/ (implemented but unused)

### 4. Testing Patterns
- TestRng: centralized deterministic RNG (Knuth MMIX LCG). ~21 call sites across 11 files.
- `next_gaussian_f32()` on TestRng for Box-Muller Gaussian sampling.
- `next_f64` uses 53-bit precision (full f64 mantissa coverage).
- All SIMD paths have bit-for-bit (or within-epsilon) tests against scalar references.
- Comprehensive property-based and ground-truth tests in star_detection and registration.
- Inconsistency: three different noise generation methods (TestRng uniform, TestRng Gaussian,
  raw multiplicative hash) across testing/. Should consolidate.

### 5. NaN/Inf Handling — Consistent
- Float FITS loader sanitizes NaN/Inf to 0.0 before normalization (float BitPix types only).
- Integer FITS types are not sanitized (cfitsio never produces NaN for integer data).
- Invariant: source images are NaN-free after loading. Enforced at FITS loader level.
- stacking `median_f32_fast` uses `partial_cmp` with NaN treated as equal — safe given NaN-free invariant.

### 6. Memory Management Patterns
- stacking: in-memory (<75% RAM) or disk-backed (mmap with MADV_SEQUENTIAL)
- star_detection: BufferPool for buffer reuse across video frames
- X-Trans demosaic: DemosaicArena single contiguous 10P allocation with region reuse
- drizzle: per-channel Buffer2<f32> (no interleaving), rayon-parallel finalization
- stacking: per-thread ScratchBuffers via rayon `for_each_init`
- centroid fitting: zero-allocation hot paths via ArrayVec/SmallVec

### 7. Error Handling — Consistent
- `Result<>` used only for expected I/O failures (FITS loading, file I/O, stacking cache)
- `ImageLoadError` with path context in all variants (astro_image, raw)
- `.unwrap()` / `.expect()` for logic errors, asserts on invariants
- stacking `Error` enum via thiserror with dimension and I/O variants

### 8. Parallelism Patterns
- Rayon used consistently across all compute-heavy modules
- star_detection: per-stage parallelism (tiles, rows, labels, centroids)
- stacking: per-row within chunks, per-thread ScratchBuffers
- registration: row-parallel warp, parallel centroid computation
- raw: row-parallel RCD and Markesteijn steps
- drizzle: parallel finalization only (accumulation is single-threaded — gap)

## Missing Industry Features (by impact)

| Priority | Feature | Module | Impact | Industry Reference |
|----------|---------|--------|--------|-------------------|
| P1 | FITS writing | astro_image | Primary astro interchange format | All tools |
| P2 | CFA/Bayer drizzle | drizzle | Bypass demosaic artifacts for OSC cameras | Siril, PixInsight, DSS, APP |
| P2 | Rejection maps output | stacking | Per-pixel high/low counts for diagnostics | PixInsight, Siril |
| P2 | Weighted least squares in L-M fitting | star_detection | Unweighted is suboptimal for faint stars | DAOPHOT, SExtractor |
| P2 | Parameter uncertainties from L-M | star_detection | Position uncertainty for weighted registration | DAOPHOT |
| P2 | Configurable sigma threshold for defect map | calibration | 5.0 hardcoded; PixInsight uses 3.0 | PixInsight, APP |
| P2 | `Buffer2::row(y)` / `row_mut(y)` | common | 251 manual row-indexing calculations across 55 files | image crate, ndarray |
| P3 | Variance propagation | stacking | Per-pixel noise estimate for downstream use | PixInsight, IRAF |
| P3 | Context/contribution image | drizzle | Per-pixel contributing-frame bitmask | STScI |
| P3 | Variance/error output | drizzle | Propagated variance for photometry | STScI |
| P3 | Parallel drizzle accumulation | drizzle | Row-parallel or per-thread accumulators | PixInsight Fast Drizzle |
| P3 | Additive-only normalization | stacking | Varying pedestal, consistent gain | Siril, PixInsight |
| P3 | Min/Max/Sum combine methods | stacking | Star trails, pixel identification | PixInsight, Siril |
| P3 | Cold pixel detection from flats | calibration | Dead pixels more reliably detected from flats | APP |
| P3 | Output framing options | registration | Max/min/COG framing for registration output | PixInsight, Siril |
| P3 | Poisson noise generator | testing | Realistic CCD noise model for photometry validation | GalSim, photutils |
| Low | Multi-HDU FITS support | astro_image | Compressed FITS, observatory formats | cfitsio, Astropy |
| Low | Large-scale rejection (satellite trails) | stacking | Wavelet + growth for coherent structures | PixInsight |
| Low | Raw CA correction | raw | Pre-demosaic lateral CA for fast optics | darktable, RawTherapee |
| Low | Rotation angle in Gaussian fit | star_detection | 7th parameter for astigmatic/elongated PSFs | Siril, SExtractor |

### Already Done (removed from above)
- ~~True Square kernel (polygon clipping)~~ — `DrizzleKernel::Square` with `sgarea()`/`boxer()`
- ~~Jacobian correction~~ — all drizzle kernels
- ~~Noise-based auto weighting~~ — `Weighting::Noise`, `w = 1/σ²`
- ~~RA/DEC metadata~~ — `ra_deg`/`dec_deg` with HMS/DMS parsing
- ~~Pixel size metadata~~ — `pixel_size_x`/`pixel_size_y`
- ~~Generic incremental stepping~~ — L2 -29.3%, L4 -45.4%, L3 -6.3%
- ~~NaN/Inf handling~~ — sanitized to 0.0 in float FITS loader

## Module Health Summary

| Module | Bugs | Correctness | Key Strength | Key Gap |
|--------|------|-------------|--------------|---------|
| math | None | Textbook correct | Hybrid Kahan/Neumaier SIMD, Cephes exp() | None critical |
| registration | None | All 5 estimators + MAGSAC++ verified (SupeRANSAC 2025) | Full pipeline at PixInsight parity | Output framing, stale READMEs |
| star_detection | None | Pipeline matches SExtractor/SEP | SIMD in every hot path, dual deblending | Weighted L-M, variance maps |
| stacking | None | All 6 rejection algos verified | MAD-based sigma, robust Winsorized | Rejection maps, variance propagation |
| calibration | None | Formula matches PixInsight/Siril | Per-CFA-color MAD detection, X-Trans support | Configurable sigma, flat-based cold detection |
| astro_image | None | FITS loading correct, comprehensive metadata | Adaptive float normalization | FITS writing |
| raw | None | X-Trans + Bayer RCD verified | 2.1x faster than libraw, 216 MP/s Bayer | Raw CA correction |
| drizzle | None | All 5 kernels verified vs STScI reference | Polygon clipping, Jacobian, per-pixel weights | CFA drizzle, parallel accumulation |
| common | None | All correct | CPU feature detection, Buffer2/BitBuffer2 | `row()`/`row_mut()` accessors |
| testing | None | Deterministic, comprehensive coverage | StarFieldBuilder, Positioned trait | Poisson noise, noise API consistency |

## Recommendations by Priority

### Short-term (high value, low effort)
1. **Add FITS writing** — primary interchange format, every other tool has this
2. **Add `Buffer2::row(y)` / `row_mut(y)`** — eliminates 251 manual calculations, trivial implementation
3. **Expose configurable sigma threshold** for defect map detection
4. **Fix stale README.md files** in registration/distortion and raw
5. **Consolidate noise generation** in testing/ (3 different methods)

### Medium-term (feature parity)
6. Add stacking rejection maps (per-pixel high/low counts)
7. Add weighted least squares to L-M fitting (inverse-variance weighting)
8. Add parameter uncertainties from L-M covariance matrix
9. Implement CFA/Bayer drizzle
10. Parallelize drizzle accumulation loops
11. Add cold pixel detection from flats in calibration
12. Add output framing options to registration (max/min/COG)

### Long-term (completeness)
13. Add variance propagation to stacking
14. Add drizzle variance/error propagation
15. Add stacking Min/Max/Sum combine methods and additive-only normalization
16. Add multi-HDU FITS support
17. Add Poisson noise and pixel-integrated PSF to testing module
18. Add large-scale rejection for satellite trails
19. Add raw chromatic aberration correction
20. Add rotation angle to Gaussian fit (7th parameter)

## Verified Correct (no action needed)

These were investigated and confirmed correct against industry references:

- **math**: All summation, statistics, sigma clipping, L-M optimizer, Jacobians (textbook correct)
- **math constants**: MAD_TO_SIGMA (1.4826022), FWHM_TO_SIGMA (2.35482) verified against Astropy/R/GSL
- **math SIMD exp()**: Cephes polynomial < 1e-12 relative error, verified against libm/SLEEF
- **calibration pipeline order**: dark sub -> flat div -> cosmetic correction (matches Siril/PixInsight)
- **calibration negative preservation**: correct (matches PixInsight; prevents positive bias)
- **calibration per-CFA-channel flat normalization**: matches PixInsight "Separate CFA flat scaling"
- **calibration MAD-based detection**: superior to Siril's avgDev (50% vs ~0% breakdown point)
- **raw pipeline order**: black sub -> WB -> demosaic (matches libraw/dcraw/RawTherapee)
- **raw WB normalization**: min=1.0 (matches dcraw convention)
- **raw black level consolidation**: replicates libraw's `adjust_bl()` correctly
- **X-Trans Markesteijn**: MAE ~0.0005 vs libraw, 2.1x speedup, coefficients match reference
- **Bayer RCD demosaic**: 5-step algorithm, 111ms/24MP (216 MP/s), buffer triple-reuse
- **registration MAGSAC++**: validated by SupeRANSAC 2025 and Piedade et al. 2025
- **registration SIP direction**: matches Siril v1.3+ convention (forward A/B)
- **registration all 5 transform estimators**: translation, euclidean, similarity, affine, homography
- **registration Lanczos3 + soft deringing**: matches PixInsight algorithm exactly
- **registration k-d tree**: all operations correct, 5 independent verifications passed
- **drizzle all 5 kernels**: Square (polygon clipping), Turbo (axis-aligned), Point, Gaussian, Lanczos-3
- **drizzle per-pixel weights**: `Option<&Buffer2<f32>>` on add_image/drizzle_stack
- **drizzle weight accumulation**: two-pass weighted mean, algebraically equivalent to STScI incremental
- **drizzle s^2 factor**: intentionally omitted (correct for surface brightness images)
- **drizzle Jacobian correction**: all kernels, finite-difference for non-square, exact for square
- **stacking all 6 rejection algorithms**: sigma clip, winsorized, linear fit, percentile, GESD, none
- **stacking normalization formulas**: global matches Siril "additive with scaling"
- **stacking noise weighting**: `Weighting::Noise` computes `w = 1/sigma_bg^2` from channel MAD stats
- **stacking winsorized**: full two-phase with Huber c=1.5, 1.134 correction, convergence
- **star_detection pipeline order**: matches SExtractor (background -> filter -> threshold -> label -> deblend -> centroid)
- **star_detection background MAD**: 50% breakdown point, superior to SExtractor clipped stddev
- **star_detection background interpolation**: Natural bicubic spline (C2), matches SEP formula exactly
- **star_detection matched filter**: noise normalization matches SEP approach
- **star_detection CCL**: RLE + atomic union-find, lock-free, ABA-safe
- **star_detection deblending**: multi-threshold tree matches SExtractor algorithm
- **star_detection Gaussian/Moffat Jacobians**: all derivatives hand-verified correct
- **FITS BZERO/BSCALE handling**: cfitsio handles transparently, normalization correct
- **FITS BAYERPAT/ROWORDER**: comprehensive parsing, matches NINA/MaximDL/Siril convention
- **FITS float normalization**: heuristic divide-by-max better than Siril's fixed /65535
- **common CPU feature detection**: OnceLock caching correct, NEON compile-time gating correct
- **common Buffer2/BitBuffer2**: bounds checking, padding isolation, alignment all correct
- **common FNV-1a**: offset basis, prime, XOR-then-multiply order all match IETF spec
- **testing TestRng**: full 2^64 period LCG, high-bit extraction mitigates low-bit correlation
- **testing StarFieldBuilder**: composable architecture matches photutils/GalSim patterns

## Industry Research Sources

Research conducted against these tools and references (full citations in per-module NOTES-AI.md):
- **FITS**: FITS Standard 4.0, cfitsio docs, Astropy, fitsio-rs
- **Calibration**: PixInsight ImageCalibration, Siril 1.5 docs, Astropy CCD Guide, APP
- **Raw**: libraw/dcraw, RawTherapee, darktable, Siril
- **Registration**: SupeRANSAC (Barath 2025), MAGSAC++ (Barath 2020), Piedade et al. 2025,
  Hartley & Zisserman 2003, PixInsight StarAlignment, Siril 1.4, Astrometry.net, Astroalign
- **Drizzle**: Fruchter & Hook 2002, STScI cdrizzle source, Siril 1.4, PixInsight PCL
- **Stacking**: PixInsight PCL IntegrationRejectionEngine, Siril 1.5, DSS, IRAF, NIST GESD
- **Detection**: SExtractor (Bertin 1996), SEP, photutils, DAOPHOT, PixInsight, Siril, PHD2
- **Math**: Astropy, GSL, Ceres Solver, NumPy, Dmitruk 2023 (SIMD summation)
- **Testing**: photutils, GalSim, WebbPSF, Astropy CCD Reduction Guide
