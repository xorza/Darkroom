# lumos Crate - Cross-Cutting Findings Summary

Per-module details in each module's `NOTES-AI.md`. This file summarizes cross-cutting patterns and the highest-priority issues across all modules.

## Top Priority Bugs & Correctness Issues

| Module | Issue | Severity |
|--------|-------|----------|
| drizzle | Drop size formula inverted: `pixfrac / scale` should be `pixfrac * scale` | Critical |
| drizzle | Square kernel only transforms center point (is actually Turbo kernel) | Critical |
| astro_image | 3-channel FITS loaded as interleaved instead of planar | Critical |
| registration | SIP distortion correction computed but never applied during warping | Critical |
| registration | Double-inverse in warp() (correct by accident) | Critical |
| raw | Bayer demosaic not implemented (`todo!()`) - affects >95% of cameras | Critical |
| math | sigma_clip_iteration: deviation/value index mismatch after quickselect | Bug |
| astro_image | FITS integer data not normalized to [0,1] (RAW is) | P1 |
| star_detection | Matched filter noise map not scaled after convolution | P1 |
| registration | Euclidean transform estimation uses wrong method | Important |

## Cross-Cutting Patterns

### 1. Inconsistent Value Range Convention
- RAW files: normalized to [0,1] by libraw
- FITS integer files: raw ADU values (0-65535 for 16-bit)
- Calibration expects consistent ranges
- Affects: astro_image, calibration_masters, stacking

### 2. Negative Values Not Handled Consistently
- calibration_masters: dark subtraction doesn't clamp to zero
- gradient_removal: subtraction correction can produce negatives
- drizzle: no output clamping
- stacking: no post-combination clamping
- Need project-wide decision: allow negatives in linear pipeline or clamp at each stage

### 3. NaN Propagation Risk
- math: partial_cmp().unwrap() panics on NaN
- Dead pixels, division-by-zero in calibration, and unclamped operations can produce NaN
- NaN propagates silently through f32 arithmetic until it hits a comparison
- Fix: Use total_cmp throughout, or add NaN guards at pipeline stage boundaries

### 4. Numerical Stability
- math: No compensated summation (naive f32 accumulation loses precision)
- gradient_removal: Normal equations (A^T*A) squares condition number
- gradient_removal: TPS coordinates not normalized (huge scale mismatch)
- registration: No weighted least-squares in final refinement
- These compound: imprecise sums feed into imprecise statistics feed into imprecise models

### 5. MAD-based Robust Statistics Used Correctly But Inconsistently
- Correct 1.4826 factor used everywhere (good)
- calibration_masters: single-channel MAD across all CFA pixels (should be per-channel)
- stacking: GESD uses median+MAD, PixInsight uses trimmed mean+stddev
- star_detection: noise map not post-convolution scaled

### 6. Missing Industry-Standard Metadata
- FITS: no BAYERPAT, FILTER, GAIN, CCD-TEMP reading
- No FITS writing support at all
- Missing context/contribution images from drizzle
- Missing rejection maps from stacking
- These limit interoperability with PixInsight, Siril, N.I.N.A.

### 7. SIMD Coverage Gaps
- raw normalization: no AVX2 path (only SSE4.1)
- math weighted_mean: scalar only, no SIMD
- drizzle: entirely single-threaded
- calibration_masters: hot pixel correction sequential
- gradient_removal: TPS evaluation O(n*W*H) with no spatial optimization

### 8. README vs Code Drift
- registration: quality score formula differs, nonexistent directories listed
- calibration_masters: claims per-channel analysis but does single-channel
- stacking: default normalization None vs README recommending Global
- gradient_removal: wrong module path in doc example

## Recommendations by Priority

### Immediate (data corruption / wrong results)
1. Fix drizzle drop size formula
2. Fix FITS planar loading in astro_image
3. Apply SIP correction during warping in registration
4. Fix sigma_clip_iteration index mismatch in math

### Short-term (correctness improvements)
5. Implement Bayer demosaic (at minimum libraw fallback)
6. Normalize FITS integer data to [0,1]
7. Scale noise map after matched filter convolution
8. Add compensated summation to math
9. Fix Euclidean transform estimation
10. Clamp dark subtraction to zero

### Medium-term (quality & interoperability)
11. Read BAYERPAT from FITS headers
12. Add FITS writing support
13. Replace normal equations with QR/SVD in gradient_removal
14. Normalize TPS coordinates
15. Add asymmetric sigma to winsorized/GESD
16. Add per-pixel bad pixel masks to drizzle

### Long-term (completeness)
17. Add cold/dead pixel detection
18. Parallelize drizzle
19. Add rejection maps to stacking
20. Implement true Square kernel with polygon clipping
