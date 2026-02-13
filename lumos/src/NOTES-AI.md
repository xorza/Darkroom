# lumos Crate - Cross-Cutting Findings Summary

Per-module details in each module's `NOTES-AI.md`. This file summarizes cross-cutting
patterns and the highest-priority **unfixed** issues across all modules.

## Top Priority Bugs & Correctness Issues

| Module | Issue | Severity |
|--------|-------|----------|
| drizzle | Drop size formula inverted: `pixfrac / scale` should be `pixfrac * scale` | Critical |
| drizzle | Square kernel only transforms center point (is actually Turbo kernel) | Critical |
| drizzle | Gaussian kernel FWHM uses wrong formula | Critical |
| raw | Bayer demosaic not implemented (`todo!()`) - affects >95% of cameras | Critical |
| drizzle | Lanczos used without required constraints (pixfrac=1, scale=1) | Major |
| drizzle | No output clamping (Lanczos negative lobes) | Major |
| gradient_removal | Normal equations solver squares condition number | Significant |
| gradient_removal | TPS coordinates not normalized (5 orders of magnitude mismatch) | Significant |
| gradient_removal | TPS regularization scaling is arbitrary (magic constant) | Significant |
| astro_image | Float FITS data not validated or normalized (out-of-range values) | Moderate |

## Already Fixed

<details><summary>Click to expand (42 items)</summary>

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

</details>

## Cross-Cutting Patterns

### 1. Negative Values Not Handled Consistently
- calibration_masters: dark subtraction doesn't clamp to zero (intentional, matches PixInsight)
- gradient_removal: subtraction correction can produce negatives
- drizzle: no output clamping (Lanczos negative lobes)
- stacking: no post-combination clamping
- **Decision**: Preserving negatives in the linear pipeline is correct (prevents positive bias).
  Clamping should only happen at output boundaries (FITS/TIFF export, display).

### 2. Numerical Stability
- gradient_removal: Normal equations (A^T*A) squares condition number → use QR/SVD
- gradient_removal: TPS coordinates not normalized (huge scale mismatch)
- gradient_removal: TPS regularization uses magic constant unrelated to data scale
- registration affine estimator: lacks Hartley-style point normalization (homography has it)
- All other modules verified correct (math sums, registration transforms, stacking formulas)

### 3. SIMD Coverage
- raw normalization: SSE4.1 only, no AVX2 path (2x throughput available)
- drizzle: entirely single-threaded
- gradient_removal: TPS evaluation O(n*W*H) with no spatial optimization
- registration: Lanczos3 SIMD kernel uses mul+add instead of actual FMA intrinsics
- **Well-covered**: math sums, convolution, threshold mask, median filter, profile fitting,
  background interpolation, warp interpolation, X-Trans demosaic

### 4. Missing Industry Features (by impact)
| Feature | Module | Impact | Industry Reference |
|---------|--------|--------|-------------------|
| Bayer demosaic (RCD) | raw | Critical | Siril default, 4-step algorithm |
| FITS writing | astro_image | High | Primary astro interchange format |
| Rejection maps | stacking | Medium | PixInsight/Siril diagnostic output |
| Per-CFA-channel flat normalization | calibration | Medium | PixInsight "Separate CFA flat scaling" |
| Iterative sample rejection | gradient_removal | Medium | photutils sigma=3, maxiters=10 |
| Noise-based auto weighting | stacking | Medium | PixInsight inverse-variance weighting |
| Context/contribution image | drizzle | Medium | STScI 32-bit bitmask per pixel |
| Per-pixel weight maps | drizzle | Medium | STScI inverse-variance maps |
| Cold pixel detection from flats | calibration | Low | APP combines hot (dark) + cold (flat) maps |
| Coarse-grid TPS evaluation | gradient_removal | Low | PixInsight GridInterpolation |

## Module Health Summary

| Module | Bugs Found | Correctness | Key Strength | Key Gap |
|--------|-----------|-------------|--------------|---------|
| math | None | All verified correct | Hybrid Kahan/Neumaier SIMD | None |
| registration | None critical | All 5 estimators verified | MAGSAC++ (validated by SupeRANSAC 2025) | FMA intrinsics in Lanczos SIMD |
| star_detection | None remaining | Pipeline matches SExtractor | SIMD in every hot path | No weight/variance map input |
| stacking | None remaining | All 6 rejection algos verified | MAD-based sigma (more robust than DSS/Siril default) | Rejection maps, auto weighting |
| calibration | None remaining | Formula matches PixInsight/Siril | Per-CFA-color MAD detection | Per-CFA flat normalization |
| astro_image | 1 moderate | FITS loading correct | Comprehensive metadata parsing | Float FITS normalization, FITS writing |
| raw | 1 critical | X-Trans verified, pipeline correct | 2.1x faster than libraw | Bayer demosaic todo!() |
| gradient_removal | 3 significant | Algorithms correct but numerically fragile | Dual-method (polynomial + TPS) | Normal equations, TPS conditioning |
| drizzle | 3 critical | Point kernel correct, overlap correct | Projective transform support | Formula bugs, missing features |

## Recommendations by Priority

### Immediate (data corruption / wrong results)
1. Fix drizzle drop size formula (`pixfrac * scale`)
2. Fix drizzle Gaussian FWHM formula (`sigma = (pixfrac * scale) / 2.3548`)
3. Implement Bayer demosaic (RCD recommended; libraw fallback as interim)

### Short-term (correctness improvements)
4. Rename drizzle Square kernel to Turbo
5. Add drizzle Lanczos parameter validation (warn/error when pixfrac != 1 or scale != 1)
6. Add drizzle output clamping (at least [0, +inf) for Lanczos kernel)
7. Fix drizzle `min_coverage` normalization (compare against normalized weight)
8. Replace gradient_removal normal equations with QR/SVD (nalgebra)
9. Normalize gradient_removal TPS coordinates to [0,1]
10. Use data-dependent TPS regularization (MATLAB tpaps-style)
11. Increase gradient_removal sample box radius (5x5 → 11x11+)
12. Add float FITS normalization heuristic (detect range, normalize if max > 2.0)

### Medium-term (quality & interoperability)
13. Add FITS writing support
14. Add stacking rejection maps (per-pixel high/low counts)
15. Add per-CFA-channel flat normalization
16. Add iterative gradient sample rejection (sigma=3, max 10 iterations)
17. Coarse-grid TPS evaluation (evaluate on 64px grid, bilinearly interpolate)
18. Implement true drizzle Square kernel (4-corner transform + polygon clipping)
19. Parallelize drizzle (rayon row-parallel)
20. Use actual FMA intrinsics in registration Lanczos3 SIMD kernel
21. Add drizzle context image (per-pixel contributing-frame bitmask)
22. Add drizzle per-pixel weight maps
23. Fix gradient_removal division correction (mean → median)

### Long-term (completeness)
24. Add noise-based auto weighting to stacking (`w = 1/sigma_bg^2`)
25. Add drizzle Jacobian correction for non-affine transforms
26. Add drizzle variance/error propagation
27. Add stacking additive-only normalization mode
28. Add stacking Min/Max/Sum combine methods
29. Generic incremental stepping for registration interpolation (benefit Lanczos2/4/Bicubic)
30. Add cold pixel detection from flats in calibration
31. Add missing FITS metadata (RA/DEC, XPIXSZ/YPIXSZ, READOUTM, DATAMAX)

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
- **stacking all 6 rejection algorithms**: sigma clip, winsorized, linear fit, percentile, GESD, none
- **stacking normalization formulas**: global matches Siril "additive with scaling"
- **star_detection pipeline order**: matches SExtractor (background → filter → threshold → label → deblend → centroid)
- **FITS BZERO/BSCALE handling**: cfitsio handles transparently, normalization correct
- **FITS BAYERPAT/ROWORDER**: comprehensive parsing, matches NINA/MaximDL/Siril convention
