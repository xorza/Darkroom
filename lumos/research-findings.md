# Lumos Crate: Deep Research Findings

Research date: 2026-02-06. Based on thorough codebase analysis and industry best practice comparison.

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [What's Done Well](#whats-done-well)
3. [What's Done Wrong or Suboptimally](#whats-done-wrong-or-suboptimally)
4. [What Could Be Improved](#what-could-be-improved)
5. [What's Strange or Unnecessary](#whats-strange-or-unnecessary)
6. [Module-by-Module Analysis](#module-by-module-analysis)
7. [Industry Comparison](#industry-comparison)
8. [Sources](#sources)

---

## Executive Summary

Lumos is a well-engineered astrophotography image processing pipeline in Rust. The codebase demonstrates strong software engineering practices: SIMD optimization where it matters, robust statistics, comprehensive testing, and clean module separation. However, there are several areas where the implementation diverges from industry best practices or makes suboptimal choices.

**Verdict:** Production-quality code with a few algorithmic gaps (demosaicing, normalization) and some unnecessary complexity. No critical bugs found.

---

## What's Done Well

### 1. Registration Pipeline (Excellent)

The registration module is the strongest part of the crate. It implements:

- **MAGSAC++ scoring** instead of a hard inlier threshold. This is state-of-the-art — MAGSAC++ is recommended by OpenCV as "the least sensitive to the inlier threshold" and is the default in SupeRANSAC (2025). Most open-source astro tools (Siril, DeepSkyStacker) still use hard thresholds.
- **LO-RANSAC** for iterative local optimization of promising hypotheses (+10-15% inlier count).
- **Progressive sampling** (3-phase confidence-weighted) — a practical PROSAC-like approach.
- **Adaptive early termination** with proper formula: `N = log(1-conf)/log(1-w^n)`.
- **Pre-computed Lanczos LUT** (48KB, fits in L1 cache) — correct optimization.
- **Plausibility checks** (rotation/scale limits) before expensive scoring.

**Comparison to industry:**
- Siril: uses basic RANSAC with hard threshold, brute-force O(n³) triangle matching on 20 stars.
- Astroalign: uses fixed k=4 for triangles, no orientation filtering, SimilarityTransform only.
- Lumos: adaptive k, orientation filtering, all 5 transform types, MAGSAC++. Clearly superior.

### 2. Star Detection (Very Good)

- SExtractor-style multi-threshold deblending with proper contrast criterion.
- Multiple centroid methods (weighted moments, Gaussian fit, Moffat fit) covering the accuracy/speed tradeoff.
- AVX2 SIMD for PSF fitting inner loops (28 accumulators for Gaussian, 21 for Moffat).
- Proper quality metrics: sharpness, roundness, SNR, FWHM, eccentricity, Laplacian SNR.
- Bit-packed threshold masks (8x memory reduction).
- Background estimation with sigma-clipped tile statistics + bilinear interpolation.

### 3. SIMD Optimization Strategy (Good)

- Runtime dispatch with scalar fallback (correct pattern).
- SIMD applied selectively where profiling showed benefit (PSF fitting, interpolation, background stats).
- NEON tested and removed for Lanczos warping (was slower due to gather overhead) — good engineering discipline.
- AVX2 bilinear warping uses practical scalar gather (acknowledged limitation, correct assessment).

### 4. Robust Statistics (Good)

- MAD-based estimators throughout (median + MAD instead of mean + stddev).
- Proper MAD-to-sigma conversion factor (1.4826).
- Sigma-clipped statistics for background estimation, normalization, rejection.

### 5. Memory Management in Stacking (Good)

- Adaptive in-memory vs disk-backed mmap caching based on available RAM.
- Zero-copy mmap with `memmap2`.
- Adaptive chunk sizing based on available memory.
- Reusable cache files across runs.

---

## What's Done Wrong or Suboptimally

### 1. Demosaicing: Bilinear Only (Wrong for Quality)

**Current:** Simple bilinear interpolation for both Bayer and X-Trans sensors.

**Problem:** Bilinear is the lowest-quality demosaicing algorithm. It produces:
- Color fringing at edges
- Reduced spatial resolution (effectively low-pass filters the image)
- Moire patterns in structured areas

**Industry standard:**
- Siril: uses VNG or SOS (Super pixel) demosaicing
- PixInsight: VNG as default, Bayer drizzle for high-quality
- RawTherapee: AMaZE (best for low-ISO), LMMSE (best for high-ISO), RCD (best for round objects like stars)
- libraw: supports AHD, VNG, PPG, DCB, DHT, AAHD

**Counterargument from the community:** For noisy astro data, "bilinear is dumb but effective — best approach in the face of noise" (Cloudy Nights). However, this applies mainly to individual sub-exposures. For stacking, VNG or RCD would give noticeably better results.

**Recommendation:** Implement RCD (Ratio Corrected Demosaicing) — it's specifically noted as excellent for "round edges like stars" while preserving detail comparable to AMaZE. Alternatively, use libraw's built-in AHD/VNG when quality > speed.

### ~~2. Normalization: Only Global, No Additive+Scaling~~ (PARTIALLY RESOLVED)

**Implemented:** `Multiplicative` normalization mode added (`gain = ref_median / frame_median`, no offset). Best for flat frames where exposure varies (sky flats, shutter speed inconsistencies).

**Available modes now:** `None`, `Global` (additive+scaling for lights), `Multiplicative` (for flats).

**Pure additive mode skipped:** Siril offers it but recommends additive+scaling for lights in all cases. Not practically useful — `Global` already covers the light frame use case.

**Remaining:** IKSS estimators could improve robustness over median+MAD, but this is a separate enhancement (see recommendations).

### 3. Hot Pixel Correction: Per-Channel Independent (Suboptimal)

**Current:** Each channel analyzed independently. A pixel must be hot in a specific channel to be flagged.

**Problem:** Hot pixels in Bayer/X-Trans raw data only appear in one color channel by definition (each pixel is one color). After demosaicing, the hot pixel value bleeds into interpolated neighbors. Detecting after demosaic means you're catching the already-spread artifact.

**Industry standard:**
- PixInsight: Cosmetic correction operates on the raw image before demosaicing
- Siril: Detects hot pixels in master dark (raw), applies correction before demosaic

**Current behavior is acceptable** because the code operates on master darks (which are stacked raw data), and the correction uses median of neighbors. But ideally, correction should happen before demosaicing for maximum effectiveness.

### 4. Calibration Order: Missing Bias-from-Dark Subtraction

**Current pipeline:**
```
1. Subtract master bias
2. Subtract master dark  
3. Divide by normalized flat
4. Correct hot pixels
```

**Problem:** If master darks were NOT bias-subtracted during creation, step 1 + step 2 double-subtracts the bias signal. The code has a comment acknowledging this but doesn't enforce it.

**Industry standard:** Either:
- Stack darks with bias subtraction → master dark is bias-free → subtract bias then dark from lights
- Stack darks WITHOUT bias subtraction → master dark includes bias → only subtract dark from lights (skip separate bias)

**Recommendation:** Document the expected workflow more prominently. Or better: subtract bias from darks during master creation if bias frames are available.

---

## What Could Be Improved

### 1. Stacking: No Drizzle Integration

**Current:** Standard pixel-aligned stacking only.

**Drizzle** (originally developed for Hubble Deep Field) can reconstruct higher-resolution images from dithered sub-exposures. This is especially valuable for undersampled optics.

**Industry standard:**
- Siril: Supports drizzle integration (2x, 3x)
- PixInsight: Full DrizzleIntegration process
- DeepSkyStacker: Drizzle 2x, 3x

**Note:** The crate has a `drizzle/` module that appears to exist but isn't fully integrated into the stacking pipeline.

### 2. Stacking: No Weighting by Frame Quality

**Current:** Optional fixed weights per frame, but no automatic quality-based weighting.

**Industry standard (PixInsight, Siril, APP):**
- Weight by FWHM (sharper frames weighted higher)
- Weight by SNR (cleaner frames weighted higher)  
- Weight by eccentricity (rounder stars weighted higher)
- Combine: `w = SNR × (1/FWHM²) × (1/eccentricity)`

Since the crate already computes these quality metrics during star detection, implementing auto-weighting would be straightforward.

### ~~3. Lanczos Warping: No SIMD for Lanczos Kernel~~ (RESOLVED — Correct as-is)

AVX2 was tested for Lanczos and provides no benefit. The bottleneck is memory-bound random 2D pixel access (36-64 gathers per output pixel), not kernel compute. This is explicitly documented in the SIMD module header. Bilinear is SIMD-optimized (AVX2, 8 pixels/cycle); Lanczos stays scalar intentionally. The real optimization path would be cache locality (tile-based warping), not SIMD.

### 4. Background Estimation: No Iterative Source Masking by Default

**Current:** Iterative refinement (detect sources → mask → re-estimate) exists but is only enabled in the `crowded_field()` preset.

**Industry standard (SExtractor):**
- Always performs at least one iteration of source masking
- Uses κσ-clipping + mode estimation: `Mode ≈ 2.5×Median - 1.5×Mean`

For typical deep-sky images with nebulosity, the default single-pass background estimation can be biased by extended emission.

### 5. TPS (Thin Plate Spline) Distortion: Implemented but Dead Code

The `distortion/tps/` module is fully implemented but marked `#![allow(dead_code)]` and not wired into the public API. Either integrate it or remove it.

### 6. No Kappa-Sigma Mode Estimation

**Current:** Background estimation uses median as location estimator.

**SExtractor:** Uses `Mode ≈ 2.5×Median - 1.5×Mean` when the distribution is not too skewed (|mean - median| < 0.3×stddev). This is more robust for backgrounds contaminated by faint unresolved sources.

### 7. Per-Pixel Weight Copy in Weighted Stacking

In `stack.rs`, weights are copied per-pixel:
```rust
local_weights.copy_from_slice(weights); // Every pixel!
```
Weights don't change between pixels — this copy is unnecessary. The rejection functions modify values but not weights.

---

## What's Strange or Unnecessary

### 1. ~~Generation Counters Everywhere~~ (REVIEWED — Correct as-is)

`PixelGrid` and `NodeGrid` in `deblend/multi_threshold/` use generation-counter-based reset. The original assessment ("premature optimization for 64×64 tiles") was incorrect — these grids are **not** the background `TileGrid`. They're sized to star candidate bounding boxes and reused across thousands of components via `DeblendBuffers`. The generation counter avoids clearing the entire (potentially large) reused buffer on every call, and `PixelGrid` has a separate visited counter bumped per BFS call. This is a well-chosen optimization for the hot path.

### 2. ~~Separate `deviation/` and `sum/` Math Modules for Trivial Operations~~ (RESOLVED)

**Fixed:** The `deviation/` module was deleted. `abs_deviation_inplace` was inlined as a private function in `statistics/mod.rs` — the only place it was used. The `sum/` module is kept as-is since it has real SIMD implementations (AVX2/SSE/NEON).

### 3. ~~Approximate vs Exact Median in Sigma Clipping~~ (RESOLVED — Documented)

`sigma_clipped_median_mad()` uses `median_f32_approx()` (upper-middle for even-length) during iterations, then `median_f32_mut()` (exact) for the final result. The doc comment now explains the design: the bias is at most half the gap between the two middle values, negligible for the hundreds-to-thousands of pixels per tile. This is an intentional performance tradeoff, not an inconsistency.

### 4. ~~`FrameType` Used for Stacking Method Selection but Not Behavior~~ (RESOLVED — Documented)

`FrameType` is used for logging and error messages only. Doc comment now explicitly states it does not affect stacking behavior — that's controlled by `StackConfig`.

### 5. `apply_from_channel` API in Calibration

The calibration code uses `image.apply_from_channel(bias, |_c, dst, src| { ... })` with 4096-element chunks and rayon parallelism. For simple subtraction/division, this per-channel callback pattern adds overhead. A fused `image -= &bias` operator would be cleaner and likely auto-vectorize better.

---

## Module-by-Module Analysis

### Registration (Grade: A)
- MAGSAC++ + LO-RANSAC = state-of-the-art
- Triangle matching with k-d tree > Siril's brute force
- SIP distortion correction follows FITS standard
- All 5 transform types with proper estimation
- SIMD bilinear warping

**Minor gaps:** No weighted least-squares final refinement, no Tabur-style ordered triangle search (only matters for 500+ stars).

### Star Detection (Grade: A-)
- SExtractor-inspired pipeline with modern optimizations
- Comprehensive quality metrics
- 3 centroid methods covering accuracy/speed tradeoff
- Extensive SIMD optimization in fitting

**Gaps:** No wavelet-based detection (PixInsight uses multiscale analysis), no PSF modeling (photometry applications).

### Stacking (Grade: B+)
- 6 rejection algorithms (good coverage)
- Adaptive memory management (good engineering)
- Parallel row processing with rayon

**Gaps:** Only 2 normalization modes (None, Global). No drizzle in main pipeline. No auto-weighting. Per-pixel weight copy overhead.

### Calibration (Grade: B+)
- Correct calibration formula
- Hot pixel detection using MAD (robust)
- Auto-loads or creates masters

**Gaps:** No bias-from-dark workflow enforcement. Hot pixel detection after demosaic. No configurable hot pixel sigma.

### Image Loading / Demosaicing (Grade: B-)
- Good sensor detection (Bayer, X-Trans, Mono)
- SIMD normalization
- FITS support

**Major gap:** Bilinear-only demosaicing. No advanced algorithms (VNG, AHD, RCD, AMaZE).

### Math Utilities (Grade: B+)
- Correct robust statistics (MAD, sigma clipping)
- SIMD where meaningful (sum, PSF fitting)
- Good numerical stability (f64 in fitting)

**Gaps:** No IKSS estimators. Deviation module has no SIMD despite dispatch structure.

---

## Industry Comparison

| Feature | Lumos | Siril | PixInsight | DeepSkyStacker |
|---------|-------|-------|-----------|----------------|
| **Demosaicing** | Bilinear | VNG/SOS | VNG/Bayer drizzle | Bilinear/AHD |
| **Registration** | MAGSAC++ + LO-RANSAC | Basic RANSAC | StarAlignment | Triangle matching |
| **Triangle matching** | k-d tree, adaptive k | Brute force, 20 stars | Proprietary | Basic |
| **Stacking rejection** | 6 methods | 5 methods | 7 methods | 4 methods |
| **Normalization** | Global only | Add/Mult/Add+Scale | Multiple | Auto |
| **Drizzle** | Module exists, not integrated | Yes (2x,3x) | Full support | Yes (2x,3x) |
| **Auto-weighting** | Manual only | FWHM/SNR based | Full QC weighting | Basic |
| **Distortion correction** | SIP polynomial | Plate-solve + SIP | StarAlignment | None |
| **SIMD optimization** | AVX2/SSE/NEON selective | None | Proprietary | None |
| **PSF fitting** | Gaussian + Moffat L-M | Simple centroid | PSF photometry | None |
| **Background estimation** | Tiled sigma-clipped | Tiled median | DBE/ABE advanced | Simple |

---

## Prioritized Recommendations

### High Impact, Low Effort
1. ~~**Add multiplicative/additive normalization modes**~~ — RESOLVED: Multiplicative added; pure additive not needed
2. **Remove per-pixel weight copy** in weighted stacking — single line fix
3. **Wire up or remove TPS dead code** — code hygiene

### High Impact, Medium Effort
4. **Implement auto-weighting** by FWHM/SNR — quality metrics already computed
5. **Add VNG or RCD demosaicing** — significant quality improvement for final output
6. **Integrate drizzle into stacking pipeline** — module partially exists

### Medium Impact, Medium Effort
7. ~~**Add SIMD Lanczos kernel**~~ — RESOLVED: already evaluated, memory-bound not compute-bound
8. **Iterative background by default** — better for nebulous fields
9. **IKSS estimators for normalization** — more robust than median+MAD

### Low Priority
10. **Simplify generation counters** — replace with memset for small grids
11. **Flatten deviation/sum module hierarchy** — code navigation improvement
12. **Mode estimation in background** — marginal improvement for most use cases

---

## Sources

- [Siril Stacking Documentation](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)
- [Siril Registration Documentation](https://siril.readthedocs.io/en/latest/preprocessing/registration.html)
- [Siril Normalization Documentation](https://free-astro.org/siril_doc-en/co/Stacking_2.html)
- [Image Stacking Methods Compared (Clark Vision)](https://clarkvision.com/articles/image-stacking-methods/)
- [Astroalign Paper (Beroiz et al., 2020)](https://arxiv.org/pdf/1909.02946)
- [MAGSAC++ Paper (Barath et al., CVPR 2020)](https://openaccess.thecvf.com/content_CVPR_2020/papers/Barath_MAGSAC_a_Fast_Reliable_and_Accurate_Robust_Estimator_CVPR_2020_paper.pdf)
- [SupeRANSAC (2025)](https://arxiv.org/html/2506.04803v1)
- [OpenCV RANSAC Evaluation](https://opencv.org/blog/evaluating-opencvs-new-ransacs/)
- [Intel AVX Lanczos Implementation](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html)
- [Drizzle Algorithm (Wikipedia)](https://en.wikipedia.org/wiki/Drizzle_(image_processing))
- [Debayerization VNG vs AHD (Cloudy Nights)](https://www.cloudynights.com/topic/608289-debayerization-vng-or-ahd/)
- [RawTherapee Demosaicing Methods](https://rawpedia.rawtherapee.com/Demosaicing)
- [SExtractor Background Modeling](https://astromatic.github.io/sextractor/Background.html)
- [DAOStarFinder (photutils)](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
- [PixInsight Cosmetic Correction](https://chaoticnebula.com/cosmetic-correction/)
- [Flat-Field Correction (Wikipedia)](https://en.wikipedia.org/wiki/Flat-field_correction)
- [PixInsight MultiscaleMedianTransform](https://www.pixinsight.com/tutorials/mmt-noise-reduction/)
- [PixInsight StarAlignment Distortion](https://www.pixinsight.com/tutorials/sa-distortion/index.html)
