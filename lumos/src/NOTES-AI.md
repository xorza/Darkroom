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
| labeling | AtomicUnionFind capacity overflow silently ignored | P1 |
| calibration | Sigma floor fails when median=0 (bias frames → massive false positives) | P1 |

## Already Fixed (removed from priority list)

- ~~astro_image: 3-channel FITS loaded as interleaved~~ — loads planar via `from_planar_channels`
- ~~astro_image: FITS integer data not normalized~~ — `BitPix::normalization_max()` normalizes to [0,1]
- ~~astro_image: Missing BAYERPAT/FILTER/GAIN/CCD-TEMP~~ — comprehensive FITS metadata parsing
- ~~math: sigma_clip_iteration index mismatch~~ — deviations recomputed after quickselect
- ~~star_detection: Matched filter noise not scaled~~ — normalized by `sqrt(sum(K^2))`
- ~~star_detection: Hardcoded radius-1 dilation~~ — removed from detect stage
- ~~star_detection: Default 4-connectivity~~ — changed to 8-connectivity
- ~~star_detection: Unsafe mutable aliasing in mask_dilation~~ — proper `SendPtr` wrapper
- ~~star_detection: PixelGrid generation counter wrap~~ — skip-zero guard added
- ~~stacking: Linear fit single center~~ — per-pixel comparison against fitted value
- ~~stacking: Winsorized missing 1.134 correction~~ — two-phase Huber c=1.5 rewrite
- ~~registration: GammaLut~~ — replaced with closed-form `gamma_k2()`
- ~~registration: L2/L-inf tolerance mismatch~~ — search radius × √2
- ~~registration: Target point degeneracy~~ — added `is_sample_degenerate`
- ~~registration: Confidence defaults misaligned~~ — both use 0.995
- ~~registration: Missing `nearest_one()`~~ — dedicated method, used in match recovery
- ~~registration: `partial_cmp().unwrap()`~~ — replaced with `total_cmp()`
- ~~registration: SIP distortion not applied during warping~~ — `WarpTransform.apply()` applies SIP
- ~~registration: Double-inverse in warp()~~ — phantom issue; transform direction is correct
- ~~registration: Euclidean estimation wrong method~~ — verified correct constrained Procrustes
- ~~calibration: Single-channel MAD across CFA~~ — per-CFA-color statistics

## Cross-Cutting Patterns

### 1. Negative Values Not Handled Consistently
- calibration_masters: dark subtraction doesn't clamp to zero
- gradient_removal: subtraction correction can produce negatives
- drizzle: no output clamping (Lanczos negative lobes)
- stacking: no post-combination clamping
- Need project-wide decision: allow negatives in linear pipeline or clamp at each stage

### 2. Numerical Stability
- math: No compensated summation (naive f32 accumulation loses precision)
- gradient_removal: Normal equations (A^T*A) squares condition number
- gradient_removal: TPS coordinates not normalized (huge scale mismatch)
- These compound: imprecise sums → imprecise statistics → imprecise models

### 3. SIMD Coverage Gaps
- raw normalization: no AVX2 path (only SSE4.1)
- math weighted_mean: scalar only, no SIMD
- drizzle: entirely single-threaded
- gradient_removal: TPS evaluation O(n*W*H) with no spatial optimization

## Recommendations by Priority

### Immediate (data corruption / wrong results)
1. Fix drizzle drop size formula (`pixfrac * scale`)
2. Fix drizzle Gaussian FWHM formula
3. Implement Bayer demosaic (at minimum libraw fallback)
4. Fix AtomicUnionFind capacity overflow
5. Fix calibration sigma floor for median=0

### Short-term (correctness improvements)
6. Fix drizzle Lanczos parameter validation + output clamping
7. Fix drizzle `min_coverage` normalization
8. Rename Square kernel to Turbo
9. Add compensated summation to math
10. Replace gradient_removal normal equations with QR/SVD
11. Normalize gradient_removal TPS coordinates
12. Increase gradient_removal sample box radius
13. Fix gradient_removal division correction (mean → median)

### Medium-term (quality & interoperability)
14. Add FITS writing support
15. Add stacking rejection maps
16. Add auto reference frame selection
17. Fix GESD asymmetric relaxation + statistics mismatch
18. Parallelize drizzle
19. Add iterative gradient sample rejection
20. Coarse-grid TPS evaluation

### Long-term (completeness)
21. Implement true Square kernel with polygon clipping
22. Add noise-based auto weighting to stacking
23. Soft deringing threshold in registration
24. Frame-type presets for stacking
