# lumos Crate - Cross-Cutting Findings Summary

Per-module details in each module's `NOTES-AI.md`. This file summarizes cross-cutting
patterns and the highest-priority **unfixed** issues across all modules.

## Top Priority Bugs & Correctness Issues

| Module | Issue | Severity |
|--------|-------|----------|
| raw | Bayer demosaic not implemented (`todo!()`) - affects >95% of cameras | Critical |
| ~~astro_image~~ | ~~Float FITS data not validated or normalized~~ → FIXED (heuristic normalization when max > 2.0) | ~~Moderate~~ |

## Already Fixed

<details><summary>Click to expand</summary>

- astro_image: 3-channel FITS loaded as interleaved → loads planar via `from_planar_channels`
- astro_image: FITS integer data not normalized → `BitPix::normalization_max()` normalizes to [0,1]
- astro_image: Missing BAYERPAT/FILTER/GAIN/CCD-TEMP → comprehensive FITS metadata parsing
- math: sigma_clip_iteration index mismatch → deviations recomputed after quickselect
- math: No compensated summation → Neumaier scalar + Kahan SIMD hybrid
- star_detection: Matched filter noise not scaled → normalized by `sqrt(sum(K^2))`
- star_detection: Hardcoded radius-1 dilation → removed from detect stage
- star_detection: Default 4-connectivity → changed to 8-connectivity
- star_detection: Unsafe mutable aliasing in mask_dilation → proper `SendPtr` wrapper
- star_detection: PixelGrid generation counter wrap → skip-zero guard added
- star_detection: AtomicUnionFind capacity overflow → assert on overflow
- stacking: Linear fit single center → per-pixel comparison against fitted value
- stacking: Winsorized missing 1.134 correction → two-phase Huber c=1.5 rewrite
- stacking: Reference frame always frame 0 → auto-select by lowest MAD
- stacking: Large-stack sorting performance → adaptive insertion/introsort
- stacking: Cache DefaultHasher non-deterministic → FNV-1a deterministic hashing
- stacking: Cache missing source file validation → mtime sidecar files
- stacking: Cache missing madvise → MADV_SEQUENTIAL on Unix
- stacking: Missing frame-type presets → bias/dark/flat/light presets
- registration: GammaLut → replaced with closed-form `gamma_k2()`
- registration: L2/L-inf tolerance mismatch → search radius × √2
- registration: Target point degeneracy → added `is_sample_degenerate`
- registration: Confidence defaults misaligned → both use 0.995
- registration: Missing `nearest_one()` → dedicated method, used in match recovery
- registration: `partial_cmp().unwrap()` → replaced with `total_cmp()`
- registration: SIP distortion not applied during warping → `WarpTransform.apply()` applies SIP
- registration: Soft deringing → PixInsight-style soft clamping (f32 threshold, default 0.3)
- registration: Direct SVD for homography DLT → Hartley normalization + SVD
- calibration: Single-channel MAD across CFA → per-CFA-color statistics
- calibration: Sigma floor fails when median=0 → added absolute floor `1e-4`
- drizzle: Drop size formula inverted (`pixfrac / scale`) → fixed to `pixfrac * scale`
- drizzle: Square kernel mislabeled → renamed to Turbo (axis-aligned approximation)
- drizzle: Gaussian kernel FWHM wrong formula → fixed sigma = `drop_size / 2.3548`
- drizzle: Lanczos used without constraints → runtime warning when pixfrac≠1 or scale≠1
- drizzle: No output clamping → Lanczos output clamped to `[0, +inf)` in finalize
- drizzle: min_coverage compared against raw weight → compared against normalized weight
- drizzle: Interleaved pixel layout → per-channel `Buffer2<f32>` (no interleaved allocation)
- drizzle: `into_interleaved_pixels()` allocation per frame → `add_image()` reads channels directly
- drizzle: Finalization single-threaded → rayon row-parallel normalization and coverage
- drizzle: Kernel implementations duplicated 4× → extracted `accumulate()` + `add_image_radial()` with closure
- drizzle: add_image_* methods had 8-10 redundant params → refactored to read dims from AstroImage
- drizzle: Coverage averaged across channels → uses channel 0 weights only (correct: same geometry)
- astro_image: `save()` cloned entire image → `to_image(&self)` borrows pixel data
- testing: 16 inline LCG RNG implementations → `TestRng` struct in `testing/mod.rs`
- testing: DVec2/Star transform functions duplicated 6× → `Positioned` trait with generic `_impl` functions
- star_detection: `#[cfg(test)]` helpers in production code → moved to `detect_test_utils.rs` and `labeling/test_utils.rs`
- star_detection: CCL run-merge logic duplicated in sequential/parallel → `merge_runs_with_prev()` with `RunMergeUF` trait
- math: `pub mod scalar` in sum exposed implementation detail → `pub(super) mod scalar`
- astro_image: `CfaImage::demosaic()` unhelpful panic on missing cfa_type → descriptive `.expect()` message
- registration: HashSet reallocation in `recover_matches` loop → pre-allocated with `with_capacity()`, reused via `.clear()`
- star_detection: Duplicate test helpers (`to_gray_image`, `to_gray_stretched`, `mask_to_gray`) → import from `common/output/image_writer.rs`
- calibration_masters: Unused `StackableImage` and `rayon::prelude::*` imports in defect_map.rs → removed
- calibration_masters: Unnecessary `.clone().unwrap()` in DefectMap::correct() → `.as_ref().unwrap()`
- star_detection: Duplicate `use rayon::prelude::*` in threshold_mask → removed
- astro_image: Float FITS data not normalized → heuristic normalization (divide by max when max > 2.0)
- calibration: Per-CFA-channel flat normalization missing → `divide_by_normalized_cfa()` with independent R/G/B means

</details>

## Cross-Cutting Patterns

### 1. Negative Values Not Handled Consistently
- calibration_masters: dark subtraction doesn't clamp to zero (intentional, matches PixInsight)
- stacking: no post-combination clamping
- **Decision**: Preserving negatives in the linear pipeline is correct (prevents positive bias).
  Clamping should only happen at output boundaries (FITS/TIFF export, display).

### 2. Numerical Stability
- registration affine estimator: lacks Hartley-style point normalization (homography has it)
- All other modules verified correct (math sums, registration transforms, stacking formulas)

### 3. SIMD Coverage
- raw normalization: SSE4.1 only, no AVX2 path (2x throughput available)
- drizzle: accumulation loops single-threaded (finalization is rayon-parallel)
- registration: Lanczos3 SIMD kernel — no-dering path now uses FMA; dering path still mul+add (masked accumulation)
- **Well-covered**: math sums, convolution, threshold mask, median filter, profile fitting,
  background interpolation, warp interpolation, X-Trans demosaic

### 4. Missing Industry Features (by impact)
| Feature | Module | Impact | Industry Reference |
|---------|--------|--------|-------------------|
| Bayer demosaic (RCD) | raw | Critical | Siril default, 4-step algorithm |
| FITS writing | astro_image | High | Primary astro interchange format |
| Rejection maps | stacking | Medium | PixInsight/Siril diagnostic output |
| ~~Per-CFA-channel flat normalization~~ | ~~calibration~~ | ~~Medium~~ | ~~PixInsight "Separate CFA flat scaling"~~ — DONE |
| Noise-based auto weighting | stacking | Medium | PixInsight inverse-variance weighting |
| Context/contribution image | drizzle | Medium | STScI 32-bit bitmask per pixel |
| Per-pixel weight maps | drizzle | Medium | STScI inverse-variance maps |
| Cold pixel detection from flats | calibration | Low | APP combines hot (dark) + cold (flat) maps |

## Module Health Summary

| Module | Bugs Found | Correctness | Key Strength | Key Gap |
|--------|-----------|-------------|--------------|---------|
| math | None | All verified correct | Hybrid Kahan/Neumaier SIMD | None |
| registration | None critical | All 5 estimators verified | MAGSAC++ (validated by SupeRANSAC 2025) | FMA intrinsics in Lanczos SIMD |
| star_detection | None remaining | Pipeline matches SExtractor | SIMD in every hot path | No weight/variance map input |
| stacking | None remaining | All 6 rejection algos verified | MAD-based sigma (more robust than DSS/Siril default) | Rejection maps, auto weighting |
| calibration | None remaining | Formula matches PixInsight/Siril | Per-CFA-color MAD detection, per-CFA flat normalization | — |
| astro_image | None remaining | FITS loading correct | Comprehensive metadata parsing, float normalization | FITS writing |
| raw | 1 critical | X-Trans verified, pipeline correct | 2.1x faster than libraw | Bayer demosaic todo!() |
| drizzle | None remaining | All 4 kernels verified correct | Projective transform, rayon finalization | True Square kernel, Jacobian correction |

## Recommendations by Priority

### Immediate (data corruption / wrong results)
1. Implement Bayer demosaic (RCD recommended; libraw fallback as interim)

### Short-term (correctness improvements)
2. ~~Add float FITS normalization heuristic~~ — DONE
3. ~~Add per-CFA-channel flat normalization~~ — DONE

### Medium-term (quality & interoperability)
4. Add FITS writing support
5. Add stacking rejection maps (per-pixel high/low counts)
6. ~~Use actual FMA intrinsics in registration Lanczos3 SIMD kernel~~ -- DONE (no-dering path)
7. Implement true drizzle Square kernel (4-corner transform + polygon clipping)
8. Add drizzle context image (per-pixel contributing-frame bitmask)
9. Add drizzle per-pixel weight maps
10. Parallelize drizzle accumulation loops (per-thread accumulators or atomics)

### Long-term (completeness)
11. Add noise-based auto weighting to stacking (`w = 1/sigma_bg^2`)
12. Add drizzle Jacobian correction for non-affine transforms
13. Add drizzle variance/error propagation
14. Add stacking additive-only normalization mode
15. Add stacking Min/Max/Sum combine methods
16. Generic incremental stepping for registration interpolation (benefit Lanczos2/4/Bicubic)
17. Add cold pixel detection from flats in calibration
18. Add missing FITS metadata (RA/DEC, XPIXSZ/YPIXSZ, READOUTM, DATAMAX)

## Verified Correct (no action needed)

These were investigated and confirmed correct against industry references:

- **math**: All summation, statistics, sigma clipping algorithms (textbook correct)
- **calibration pipeline order**: dark sub → flat div → cosmetic correction (matches Siril/PixInsight)
- **calibration negative preservation**: correct (matches PixInsight; prevents positive bias)
- **raw pipeline order**: black sub → WB → demosaic (matches libraw/dcraw/RawTherapee)
- **raw WB normalization**: min=1.0 (matches dcraw convention)
- **X-Trans Markesteijn**: MAE ~0.0005 vs libraw, 2.1x speedup, coefficients match reference
- **registration MAGSAC++**: validated by SupeRANSAC 2025 and Piedade et al. 2025
- **registration SIP direction**: matches Siril v1.3 convention (forward A/B)
- **registration all 5 transform estimators**: translation, euclidean, similarity, affine, homography
- **registration Lanczos3 + soft deringing**: matches PixInsight algorithm exactly
- **drizzle all 4 kernels**: Turbo (axis-aligned drop), Point, Gaussian (FWHM=pixfrac*scale), Lanczos-3
- **drizzle weight accumulation**: two-pass weighted mean, min_coverage against normalized weight
- **drizzle output**: Lanczos clamped [0,+inf), rayon-parallel finalization, Buffer2 per-channel storage
- **stacking all 6 rejection algorithms**: sigma clip, winsorized, linear fit, percentile, GESD, none
- **stacking normalization formulas**: global matches Siril "additive with scaling"
- **star_detection pipeline order**: matches SExtractor (background → filter → threshold → label → deblend → centroid)
- **FITS BZERO/BSCALE handling**: cfitsio handles transparently, normalization correct
- **FITS BAYERPAT/ROWORDER**: comprehensive parsing, matches NINA/MaximDL/Siril convention
