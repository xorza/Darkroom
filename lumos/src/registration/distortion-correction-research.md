# Reducing Registration RMS Error: Distortion Correction Approaches

## Current State

- **Pipeline:** Triangle matching → RANSAC → Homography (8 DOF)
- **RMS error:** 0.26px (with `inlier_threshold=0.5`, 35 inliers)
- **Residual distribution:** min=0.05px, median=0.25px, max=0.59px
- **Bottleneck:** Spatially-varying optical distortion that a single global homography cannot capture

The centroid measurement precision (Moffat fit) is ~0.01px, but residuals are 10-50x larger — the error is dominated by the transform model, not measurement noise.

## Approach 1: SIP Polynomial Distortion (Industry Standard)

The [SIP (Simple Imaging Polynomial)](https://fits.gsfc.nasa.gov/registry/sip/SIP_distortion_v1_0.pdf) convention is the standard in astronomy for representing non-linear geometric distortion. Used by Spitzer, HST, [Astrometry.net](https://astrometry.net/doc/readme.html), [Siril](https://siril.readthedocs.io/en/latest/preprocessing/registration.html), and ASTAP.

**How it works:**
- Pixel coordinates (u, v) relative to a reference point are corrected before the linear (CD matrix) transform
- Correction is a 2D polynomial: `u' = u + f(u,v)`, `v' = v + g(u,v)` where f, g are polynomials of order 2-5
- No terms below quadratic (linear distortion is in the CD matrix / homography)
- Order 2 (6 extra coefficients) handles barrel/pincushion; order 3 (10 extra) handles mustache distortion

**Expected improvement:**
- [Siril docs](https://siril.readthedocs.io/en/stable/astrometry/platesolving.html) report SIP order 3-5 corrects most optical distortion
- 5th order achieves median residual of **0.011 pixels** on HST data
- For consumer optics (our case), order 2-3 should bring RMS from 0.26px to ~0.05-0.10px

**Implementation in our pipeline:**
1. After RANSAC finds the homography + inliers, fit a 2D polynomial to the residuals
2. The polynomial maps `(x, y) → (dx, dy)` correction vectors
3. Apply correction before the homography
4. Re-run RANSAC or least-squares with the corrected model
5. Store as `Transform::SipCorrected { homography, sip_forward, sip_inverse }`

**Pros:**
- Well-understood, standard convention (FITS WCS compatible)
- Moderate implementation effort — just polynomial fitting on residuals
- Order 2 adds only 6 parameters; order 3 adds 10
- Doesn't require many matched stars (20-30 sufficient for order 3)

**Cons:**
- Higher-order polynomials can diverge at image edges (only regular polynomials, not Chebyshev)
- Fixed polynomial order — may underfit complex distortion or overfit with few stars
- Reverse transform is approximate (not exact inverse)

**Complexity:** Medium. Core is a polynomial least-squares fit on residual vectors.

## Approach 2: Thin Plate Splines (PixInsight's Approach)

[PixInsight StarAlignment](https://www.pixinsight.com/tutorials/sa-distortion/index.html) uses **approximating thin plate splines** (TPS) as their distortion model, achieving sub-0.03px accuracy.

**How it works:**
- A thin plate spline describes the minimal-energy bending of a thin metal sheet through control points
- Given N matched star pairs, compute a smooth surface that maps reference positions to target positions
- **Approximating** (not interpolating) splines — doesn't reproduce every point exactly, assumes measurement noise
- Smoothing parameter controls rigidity: high = global transform, low = local flexibility

**Key insight from PixInsight:**
- Early versions used **interpolating** splines (exact fit to all points) — this overfitted to centroid errors
- Switched to **approximating** splines — much more robust, assumes star positions have uncertainty
- Recent versions use DDM (Domain Decomposition Method) thin plate splines via ALGLIB for O(n·log(n)) performance instead of classical O(n³)
- [Surface simplification](https://pixinsight.com/tutorials/solver-distortion/) reduces control points by ~86% while maintaining accuracy

**Expected improvement:**
- PixInsight reports consistent **<0.03px** residuals across entire images
- Should bring our 0.26px down to ~0.03-0.05px

**Implementation in our pipeline:**
1. After RANSAC, collect all inlier matched pairs
2. Build a TPS with smoothing parameter (regularization)
3. The TPS directly maps `(x, y) → (x', y')` including both the linear part and distortion
4. Replaces the homography entirely as the transform model
5. For pixel remapping during stacking, evaluate TPS at each output pixel

**Pros:**
- Extremely flexible — handles arbitrary distortion patterns (barrel, coma, field curvature, etc.)
- No need to choose polynomial order — adapts to the data
- Approximating variant handles measurement noise naturally
- Proven in production (PixInsight has used this for 10+ years)

**Cons:**
- Classical TPS is O(n³) for N control points — expensive for >1000 stars
- Need to choose smoothing parameter (too low = overfit, too high = underfit)
- More complex to implement than polynomial fitting
- Inverse transform requires iterative solving (Newton's method)
- Dense linear system (unlike sparse polynomial)

**Complexity:** High. Requires implementing the TPS kernel, regularized linear solve, and parameter tuning.

## Approach 3: Piecewise Affine (Delaunay Triangulation)

Used by the [starmatch](https://pypi.org/project/starmatch/) package and scikit-image's `PiecewiseAffineTransform`.

**How it works:**
- Triangulate the matched star pairs using Delaunay triangulation
- Each triangle defines a local affine transform (6 DOF per triangle)
- For any pixel, find which triangle it falls in, apply that triangle's local affine
- Produces a continuous (C0) piecewise-linear mapping

**Expected improvement:**
- Residuals at control points are exactly 0 (interpolating)
- Between control points, accuracy depends on star density and distortion smoothness
- Likely 0.05-0.15px RMS depending on star distribution

**Implementation in our pipeline:**
1. After RANSAC, take all inlier pairs as control points
2. Compute Delaunay triangulation of reference positions
3. For each triangle, compute the affine transform from its 3 ref vertices to 3 target vertices
4. To transform a pixel: find containing triangle → apply its local affine

**Pros:**
- Simple to implement (Delaunay + per-triangle affine)
- Naturally adapts to local distortion
- Fast evaluation (point-in-triangle lookup + 2x3 matrix multiply)
- No parameter tuning needed
- Handles arbitrary distortion if stars are well-distributed

**Cons:**
- C0 continuity only — visible seams at triangle boundaries in warped images
- Interpolating (not approximating) — overfits to centroid noise
- Requires good spatial distribution of matched stars
- Extrapolation outside convex hull is undefined
- Not suitable for mosaics or images with sparse star coverage

**Complexity:** Low-Medium. Delaunay triangulation is the main algorithmic component.

## Approach 4: Local Weighted Mean Transform

**How it works:**
- For each pixel to transform, find K nearest matched star pairs
- Compute a local affine transform from those K pairs, weighted by distance
- Closer stars get higher weight (Gaussian or inverse-distance weighting)
- Results in a smooth, spatially-varying transform

**Expected improvement:**
- Similar to TPS but with explicit locality — 0.05-0.10px achievable

**Implementation in our pipeline:**
1. After RANSAC, build a k-d tree of inlier reference positions
2. To transform pixel (x, y): find K nearest matched pairs
3. Compute weighted least-squares affine from those K pairs
4. Apply the local affine to (x, y)

**Pros:**
- Smooth output (no triangle seams)
- Naturally handles spatially-varying distortion
- Easy to understand and implement
- No global solve needed — fully local

**Cons:**
- Expensive per-pixel evaluation (K-nearest search + weighted LS for every pixel)
- Choice of K and weighting function affects results
- Can produce artifacts in regions with few stars
- Not invertible analytically

**Complexity:** Medium. K-d tree + per-pixel weighted least-squares.

## Approach 5: Two-Stage RANSAC + Iterative Refinement

A simpler refinement of the current approach without changing the transform model.

**How it works:**
1. Run RANSAC to find homography + inliers (current approach)
2. Refit the homography using only inliers via least-squares (not just the RANSAC minimal set)
3. Recompute inliers with tighter threshold
4. Iterate 2-3 times

This is essentially what [LO-RANSAC](https://en.wikipedia.org/wiki/Random_sample_consensus) (Local Optimization RANSAC) does, and we already have it enabled (`use_local_optimization: true`).

**Expected improvement:**
- Marginal (~5-10% RMS reduction) since LO-RANSAC is already active
- Cannot fix the fundamental model limitation (homography can't represent radial distortion)

**Complexity:** Already implemented.

## Recommendation

### Pragmatic path: SIP Polynomial (Approach 1)

**Start with SIP order 3** — it's the industry standard, moderate to implement, and should reduce RMS from 0.26px to ~0.05-0.10px. The implementation is:

1. Run current pipeline: triangle matching → RANSAC → homography
2. Collect inlier residual vectors: `residual_i = transform(ref_i) - target_i`
3. Fit 2D polynomials to residual x and y components: `dx = Σ a_ij · x^i · y^j`, `dy = Σ b_ij · x^i · y^j` (for i+j ≤ order)
4. Store combined transform: first apply polynomial correction, then homography
5. Optionally iterate: recompute inliers with corrected model, refit

This requires ~20 lines of polynomial fitting code (already have a linear solver in the centroid module) and minimal changes to the Transform struct.

### If more accuracy needed: Approximating TPS (Approach 2)

If SIP doesn't reach the target, upgrade to approximating thin plate splines. This is what PixInsight uses and achieves <0.03px. More complex but well-proven. Consider using a Rust RBF/spline library rather than implementing from scratch.

### Not recommended: Piecewise Affine (Approach 3)

The C0 discontinuities at triangle edges would cause visible artifacts in stacked images. Only suitable if you don't warp pixels (just need star position mapping).

## Sources

- [SIP Convention for FITS Distortion](https://fits.gsfc.nasa.gov/registry/sip/SIP_distortion_v1_0.pdf) — Shupe et al. 2005
- [PixInsight StarAlignment Distortion Correction](https://www.pixinsight.com/tutorials/sa-distortion/index.html)
- [PixInsight Plate Solving Distortion Algorithm](https://pixinsight.com/tutorials/solver-distortion/)
- [Siril Registration Documentation](https://siril.readthedocs.io/en/latest/preprocessing/registration.html)
- [Siril Plate Solving with SIP](https://siril.readthedocs.io/en/stable/astrometry/platesolving.html)
- [Astroalign: Python Astronomical Image Registration](https://arxiv.org/pdf/1909.02946)
- [starmatch PyPI — Radial & Piecewise Distortion Models](https://pypi.org/project/starmatch/)
- [Astrometry.net Documentation](https://astrometry.net/doc/readme.html)
- [HNSKY SIP Coefficients Usage](https://www.hnsky.org/sip.htm)
- [Astro Pixel Processor — Dynamic Distortion Correction](https://www.astropixelprocessor.com/with-unique-features/)
