# registration Module - Implementation Status

Complete astronomical image registration pipeline: triangle asterism matching via k-d tree, RANSAC with MAGSAC++ scoring and LO-RANSAC, transform models (Translation through Homography), optional SIP distortion correction, image warping with Lanczos interpolation.

## Architecture Strengths
- Triangle formation via k-d tree O(n*k^2) with adaptive k selection vs O(n^3) brute-force
- K-d tree on Valdes (1995) invariant space - canonical geometric hash approach
- MAGSAC++ scoring (Barath & Matas 2020) - state of the art inlier evaluation
- FWHM-adaptive sigma for inlier threshold
- DLT with Hartley normalization for homography
- Post-RANSAC match recovery via k-d tree projection (recovers 10-30% more matches)
- Dense/sparse vote matrix auto-switch at 250K entries
- Flat implicit k-d tree (8 bytes/node vs ~40 with pointers)
- SIMD-accelerated interpolation (AVX2 bilinear, 8 pixels/cycle)
- Lanczos LUT (48KB, fits L1 cache)
- 5 well-calibrated presets (fast, precise, wide_field, mosaic, precise_wide_field)

## Critical Issues

### Double-Inverse in warp()
**Files:** `mod.rs:185`, `interpolation/mod.rs:237`

`warp()` computes `transform.inverse()` then passes to `warp_image()`, which inverts again. This applies the forward transform (correct by accident via double negation) but wastes a matrix inversion and creates deep confusion.

**Fix:** Remove inverse in `warp()`, pass transform directly.

### SIP Correction Not Applied During Warping
**File:** `mod.rs:178-188`

`warp()` ignores `RegistrationResult.sip_correction` entirely. SIP polynomial is fit and stored but never used in actual warping - only for computing residuals (`mod.rs:205-208`). The primary purpose of SIP (correcting distortion in warped image) is not achieved.

### No Inverse SIP Polynomial (AP/BP Coefficients)
**File:** `distortion/sip/mod.rs`

SIP standard defines AP_pq/BP_pq for inverse direction. README mentions `compute_inverse()` but it doesn't exist. Cannot efficiently go from corrected coords to pixel coords (needed for warping).

**Fix:** Implement via grid sampling + least-squares fitting (LSST uses 100x100 grid).

### Euclidean Transform Estimation is Incorrect
**File:** `ransac/transforms.rs:60-65`

Fits similarity (allows scale), extracts rotation+translation, creates Euclidean (scale=1). Translation from similarity fit incorporates the fitted scale. Forcing scale=1 with same translation is inconsistent.

**Fix:** Implement proper constrained Procrustes (Horn's method with scale fixed at 1).

## Important Issues

### No Sigma-Clipping in SIP Fitting
README acknowledges as pending improvement. Marginal RANSAC inliers can disproportionately affect polynomial fit. LSST uses 3 iterations of 3-sigma clipping.

### TPS Not Integrated Into Pipeline
**File:** `distortion/tps/mod.rs` - marked `#![allow(dead_code)]`

Implementation exists and is tested but not accessible from public API. PixInsight's primary distortion correction uses TPS iteratively.

### README vs Code Inconsistencies
- Quality score formula in `result.rs:109-116` uses 2-component formula
- README describes 4-component formula with different scaling
- Module structure in README lists directories that don't exist (pipeline/, phase_correlation/, etc.)

## Minor Issues

### random_sample_into Allocates O(n) Every RANSAC Iteration
**File:** `ransac/mod.rs:337-345`

`buffer.extend(0..n)` called 2000 times with n~200. Could pre-allocate and persist index buffer.

### No Weighted Least-Squares in Final Transform Refinement
**File:** `ransac/mod.rs:248-262`

Uses unweighted LS on all inliers. Points with higher MAGSAC++ scores should contribute more.
