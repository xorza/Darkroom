# Distortion Module - Research Analysis

## Overview

This module provides two distortion correction models for astronomical image
registration:

1. **SIP Polynomial** (`sip/mod.rs`): Parametric polynomial distortion following
   the FITS WCS SIP convention (Shupe et al. 2005). Integrated into the
   registration pipeline.
2. **Thin Plate Spline** (`tps/mod.rs`): Non-parametric radial basis function
   interpolation. Implemented but not yet integrated (marked `dead_code`).

---

## SIP Correctness Analysis

### Direction Convention: Forward vs Inverse

The SIP standard defines two polynomial sets:

- **A/B (Forward)**: Pixel-to-sky direction. Given pixel offsets (u, v) from
  CRPIX, computes corrected pixel coordinates before applying the CD matrix:
  `U = u + Sum A_pq * u^p * v^q`, `V = v + Sum B_pq * u^p * v^q`.
- **AP/BP (Inverse)**: Sky-to-pixel direction. Given intermediate world
  coordinates, computes the pixel position. These are approximate inverses of
  A/B, fit separately.

**What this implementation computes**: The code at `sip/mod.rs:148-158`
computes residuals as `-(transform.apply(ref) - target) / norm_scale`, then
fits polynomial coefficients that map `ref -> corrected_ref` such that
`transform(corrected_ref) ~ target`. This is effectively the SIP **forward**
polynomial (A/B): it corrects pixel coordinates before applying the linear
transform.

**What the warp pipeline needs**: For output-to-input warping, `WarpTransform::apply`
(transform.rs:346-352) computes `src = transform.apply(sip.correct(p))` where
`p` is the output pixel. This means SIP correction is applied to the output
(reference) frame pixel position before the linear transform maps it to the
source frame. This is the correct direction: the forward SIP correction adjusts
pixel coordinates in the reference frame, then the homography/affine maps them
to the target (source) frame.

**Verdict**: Direction is correct. The implementation fits forward (A/B)
polynomials and applies them before the linear transform, consistent with the
SIP standard and how tools like Astrometry.net, STWCS, and MOPEX use A/B.

### Reference Point Handling

`sip/mod.rs:142-145`: When `reference_point` is None, the centroid of
`ref_points` is used. When specified, it acts as CRPIX (the FITS reference
pixel).

The SIP standard mandates that coordinates be relative to CRPIX. Using the
centroid as default is pragmatic for registration (no FITS header available),
but can produce worse fits when distortion is radial from the optical axis
(which is typically near image center, not star centroid). This is demonstrated
in `tests.rs:300-370` (`test_reference_point_crpix_vs_centroid`).

The pipeline at `registration/mod.rs:318` passes `reference_point: None`,
always using the centroid. This means it never uses the image center as
reference even when that information could be available.

**Issue**: For best results with typical optical distortion (radial from image
center), the pipeline should pass the image center as reference point.

### Coordinate Normalization

`sip/mod.rs:146,260-263`: Coordinates are normalized by the average distance
from points to the reference point. This is good practice for polynomial fitting
(reduces condition number of normal equations). The coefficients are stored in
normalized space, and `correction_at` (line 219-232) denormalizes when applying.

This is an improvement over the SIP standard itself, which does not specify
normalization. LSST uses a "scaled polynomial transform" approach that is
"somewhat more numerically stable than the SIP form" for similar reasons.

### Normal Equation Assembly

`sip/mod.rs:270-308`: The `build_normal_equations` function correctly builds
A^T*A and A^T*b for the least-squares system. The symmetric matrix is filled
by accumulating `basis[j] * basis[k]` for each point. The upper triangle is
computed first, then mirrored (lines 297-300). This is standard and correct.

Both U and V axes share the same A^T*A matrix (since the design matrix depends
only on point positions, not targets). The code correctly computes a single
A^T*A and two separate A^T*b vectors (lines 302-303), solving once for each
axis. This is a common and valid optimization.

### Polynomial Term Generation

`sip/mod.rs:241-250`: The `term_exponents` function generates monomials with
`2 <= p+q <= order`, correctly excluding linear terms (those are in the
homography/CD matrix). The iteration order is `total` then `p` descending,
producing: (2,0), (1,1), (0,2), (3,0), (2,1), (1,2), (0,3), etc. This matches
the SIP standard.

**Term count verification** (line 42): `MAX_TERMS = 18` for order 5.
Formula: `Sum_{k=2}^{order} (k+1) - 3` = `(order+1)(order+2)/2 - 3`.
For order 5: `6*7/2 - 3 = 18`. Correct.

### Solver Choice

**Cholesky** (`sip/mod.rs:313-358`): Primary solver for the normal equations.
A^T*A is symmetric positive definite (SPD) when the design matrix has full rank,
making Cholesky the optimal choice: O(n^3/3) operations, numerically stable for
well-conditioned SPD matrices, no pivoting needed.

**LU fallback** (`sip/mod.rs:362-418`): When Cholesky fails (non-positive
definite, which can happen with near-degenerate point configurations), the code
falls back to Gaussian elimination with partial pivoting. This is appropriate.

**Comparison with alternatives**:
- **SVD**: More robust for ill-conditioned systems. The condition number of
  A^T*A is the square of the condition number of A, so SVD on A directly would
  be more stable. However, with coordinate normalization (avg distance -> 1.0),
  the condition number is kept manageable for typical SIP orders (2-5).
- **QR**: Also more stable than normal equations for ill-conditioned cases.
- In practice, Cholesky with LU fallback and coordinate normalization is
  adequate for the SIP use case. The GSL documentation notes "for low to
  moderate condition numbers the normal equation method with Cholesky is
  preferable."

**Missing**: No condition number check or warning. If the normal equations are
severely ill-conditioned (e.g., too many terms for the number/distribution of
points), the solution silently degrades. A condition number estimate (ratio of
largest to smallest diagonal of L) would be cheap to compute during Cholesky.

### Sigma-Clipping

`SipConfig` declares `clip_sigma: f64` and `clip_iterations: usize` (lines
66-71) with defaults of 3.0 and 3 respectively. However, **these fields are
never used** in `fit_from_transform`. The fitting proceeds in a single pass
with no outlier rejection.

This is a significant gap. The LSST pipeline (`FitSipDistortionTask`) performs
iterative sigma-clipping during SIP fitting, as marginal RANSAC inliers can
disproportionately affect polynomial coefficients. The LSST approach:
1. Fit polynomial
2. Compute residuals
3. Reject points beyond N*sigma (using MAD-based sigma estimate)
4. Refit with remaining points
5. Repeat for K iterations

The config fields exist but the implementation is missing. This is dead code
that creates a false API promise.

### max_correction() Method

`sip/mod.rs:203-216`: Samples a grid of points across the image and returns
the maximum correction magnitude. This is a diagnostic/sanity-check method.
The implementation is correct but could be more useful if it also returned the
location of the maximum correction. Useful for detecting overfitting (if
max_correction is unreasonably large, the polynomial may be extrapolating
wildly).

---

## TPS Correctness Analysis

### Kernel Function

`tps/mod.rs:229-231`: `U(r) = r^2 * ln(r)` with `U(0) = 0`.

This is the standard 2D thin-plate spline kernel. The limit
`lim_{r->0} r^2 ln(r) = 0` is correct and handled by the `r < 1e-10` check.
Wikipedia and Bookstein (1989) confirm this kernel for 2D TPS.

### System Assembly

`tps/mod.rs:85-113`: The system matrix has the structure:

```
[K + lambda*I  P] [w]   [v]
[P^T           0] [a] = [0]
```

where K[i,j] = U(||p_i - p_j||), P[i,:] = [1, x_i, y_i].

This is the standard TPS formulation. The code correctly:
- Sets diagonal of K to regularization (line 92), equivalent to K + lambda*I
- Fills P and P^T symmetrically (lines 101-109)
- Sets RHS to target positions for the first N entries (lines 118-121)
- Sets last 3 entries of RHS to 0 (already initialized)

**Correct**. Matches the standard Bookstein (1989) formulation.

### Regularization

`TpsConfig.regularization` (default 0.0) controls the smoothing parameter
lambda. This is correctly applied as the diagonal of the K block (line 92).

PixInsight uses regularization by default (spline smoothness = 0.25) because
measured star positions have errors. For exact interpolation (lambda = 0),
the TPS passes through all control points exactly but may oscillate between
them. With regularization, the TPS trades off interpolation accuracy for
smoothness.

The test `test_tps_regularization` (tps/tests.rs:220-259) verifies that
regularization reduces bending energy, and `test_tps_regularization_values`
(tests.rs:533-575) verifies monotonically decreasing energy with increasing
lambda. Both tests pass.

### Solver

`tps/mod.rs:234-293`: Uses LU decomposition with partial pivoting via
`solve_linear_system`. The implementation uses `Vec<Vec<f64>>` for the
augmented matrix, which is allocation-heavy but correct.

The TPS matrix is not positive definite (the lower-right 3x3 block is zeros),
so Cholesky cannot be used. LU with partial pivoting is appropriate.

**Performance concern**: The `Vec<Vec<f64>>` allocation pattern creates
O(n) heap allocations for an (n+3)-row matrix. For large n, a contiguous
allocation (single `Vec<f64>` with manual indexing, as done in the SIP solver)
would be more cache-friendly.

### Why dead_code?

`tps/mod.rs:2`: `#![allow(dead_code)]` -- "WIP: TPS distortion modeling is
not yet integrated into the public API."

The module is fully implemented and well-tested (16 tests covering exact
interpolation, affine transforms, barrel distortion, regularization, edge
cases, and distortion maps). The `dead_code` suppression is because TPS is not
used by any public-facing code path.

`tps_kernel` is exported as `pub(crate)` (mod.rs:33) but no crate-internal
code uses it either.

### Integration Requirements

To integrate TPS into the pipeline:
1. Add TPS as an alternative to SIP in `RegistrationConfig`
2. After RANSAC, fit TPS to inlier correspondences (source = ref positions,
   target = where they should map to after removing the linear transform)
3. Bundle TPS into `WarpTransform` (currently only supports SipPolynomial)
4. Apply TPS correction in `WarpTransform::apply`
5. Consider: TPS maps source -> target directly, while SIP is a correction
   added to coordinates. The integration needs to handle this difference.

### Performance Characteristics

- **Solve**: O(n^3) for the (n+3) x (n+3) system. For n=100 control points,
  this is ~1M operations -- negligible. For n=1000, ~1B operations -- may need
  optimization (e.g., H-matrix approximation).
- **Evaluate**: O(n) per point (sum over all control points). For warping a
  4K image with 100 control points, that's ~100 * 8M = 800M kernel evaluations.
  The `ln()` call in the kernel makes this expensive. A `DistortionMap` (grid
  pre-evaluation + bilinear interpolation) would be much faster for warping.

---

## Issues Found

### Critical

1. **Sigma-clipping config fields are dead code** (`sip/mod.rs:66-71`).
   `clip_sigma` and `clip_iterations` are declared in `SipConfig` with defaults
   but never used in `fit_from_transform`. The LSST pipeline performs iterative
   sigma-clipping during SIP fitting; this is industry standard practice.

### Important

2. **Pipeline always uses centroid, never image center** (`registration/mod.rs:318`).
   The SIP standard uses CRPIX (typically image center) as the reference point.
   When distortion is radial from the optical axis, using the centroid of
   detected stars (which depends on field coverage) produces suboptimal fits.
   The pipeline should accept an optional image center parameter.

3. **No condition number monitoring** (`sip/mod.rs:313-358`). The Cholesky
   solver does not check or warn about ill-conditioning. For high-order
   polynomials with few points or clustered points, the normal equations can
   become severely ill-conditioned. A simple check: if any diagonal element of
   L is very small relative to the largest, warn or fall back to SVD.

4. **No polynomial order validation against point count**. The only check is
   `n < terms.len()` (line 138), requiring at least as many points as terms.
   In practice, you need significantly more points than terms to avoid
   overfitting. Astrometry.net warns that "higher orders may produce totally
   bogus results." A ratio of at least 3:1 (points:terms) is a common
   heuristic. For order 5 (18 terms), you would need at least 54 points.

### Minor

5. **TPS solver uses Vec<Vec<f64>>** (`tps/mod.rs:242-250`). Each row is a
   separate heap allocation. A flat `Vec<f64>` with `row * (n+1)` indexing
   (as used in the SIP LU solver) would be more efficient.

6. **TPS kernel evaluates ln() per call** (`tps/mod.rs:229-231`). For warping,
   this is called millions of times. Pre-computing a distortion grid
   (`DistortionMap`) and interpolating is the standard approach (PixInsight
   does this).

7. **No inverse polynomial support**. The SIP standard defines AP/BP inverse
   polynomials for sky-to-pixel mapping. The README.md mentions
   `compute_inverse` and related methods, but these are not present in the
   current code. This is needed for FITS header export and for some warping
   configurations.

---

## Missing Features

### vs LSST Pipeline
- Iterative sigma-clipping during SIP fitting (LSST uses 3 iterations)
- Intrinsic scatter estimation
- Fitting reverse transform (AP/BP) first, then deriving forward from grid
  (more numerically stable per LSST documentation)
- BIC/AIC for automatic order selection

### vs Astrometry.net
- Automatic polynomial order selection (Astrometry.net defaults to order 2,
  adjusts based on field size and star count)
- Match-weighting during polynomial fitting (downweighting uncertain matches)
- SIP header export (A_ORDER, A_p_q keywords)

### vs PixInsight
- TPS integration into registration pipeline
- Approximating splines with regularization (PixInsight default smoothness 0.25)
- Iterative distortion refinement (PixInsight uses up to 100 iterations)
- Distortion grid pre-computation for efficient warping

### General
- FITS WCS SIP header import/export
- Cross-validation for order selection
- Weighted least squares using match quality/brightness
- Residual visualization / distortion vector field output
- Brown-Conrady radial+tangential model as alternative (simpler, fewer params
  for purely radial distortion)

---

## Potential Improvements

1. **Implement sigma-clipping** in `fit_from_transform` using the existing
   config fields. Compute residuals after initial fit, estimate sigma via MAD,
   reject outliers, refit. 3 iterations with 3-sigma threshold.

2. **Pass image center as reference_point** in the pipeline when available.
   Add an `image_size: Option<(usize, usize)>` to `RegistrationConfig` and
   use `(width/2, height/2)` as `reference_point`.

3. **Add condition number warning** to the Cholesky solver. Compute
   `max(diag(L)) / min(diag(L))` and log a warning if it exceeds ~1e10.

4. **Integrate TPS** as an alternative distortion model. Use `DistortionMap`
   for efficient warping (pre-compute on grid, bilinear interpolate during warp).

5. **Add order validation heuristic**: warn or cap order if
   `n_points < 3 * n_terms`. This prevents overfitting without being too
   restrictive.

6. **Flatten TPS solver allocation**: Replace `Vec<Vec<f64>>` with
   `Vec<f64>` using `row * stride + col` indexing.

---

## SIP vs TPS: When to Use Which

| Aspect | SIP Polynomial | Thin Plate Spline |
|--------|---------------|-------------------|
| Model type | Global parametric | Local non-parametric |
| Best for | Optical distortion (radial) | Complex, non-uniform distortion |
| Parameters | 3-18 per axis (order 2-5) | N+3 per axis (N = control points) |
| Overfitting risk | High at order >= 4 | Low with regularization |
| Extrapolation | Diverges outside data | Extrapolates linearly (affine) |
| FITS standard | Yes (SIP keywords) | No |
| Speed (evaluate) | O(terms) ~ns | O(N) ~us, or O(1) with grid |
| Industry use | Astrometry.net, Siril, HST | PixInsight, medical imaging |

**Recommendation**: Use SIP order 3 as default (handles barrel/pincushion).
Use TPS for wide-field mosaics with complex distortion patterns, or when
SIP residuals remain high. PixInsight defaults to TPS; Astrometry.net
defaults to SIP order 2.

---

## File Structure

```
distortion/
  mod.rs           Re-exports: SipConfig, SipPolynomial, TpsConfig,
                   ThinPlateSpline, DistortionMap, tps_kernel
  NOTES-AI.md      This file
  README.md        Human-readable overview
  sip/
    mod.rs         SipPolynomial fitting + evaluation (419 lines)
    tests.rs       10 tests: barrel, pincushion, orders, edge cases, CRPIX
  tps/
    mod.rs         ThinPlateSpline, DistortionMap, tps_kernel (398 lines)
    tests.rs       16 tests: interpolation, transforms, regularization, edge cases
```

## References

- [SIP Convention (Shupe et al. 2005)](https://irsa.ipac.caltech.edu/data/SPITZER/docs/files/spitzer/shupeADASS.pdf)
- [SIP FITS Registry](https://fits.gsfc.nasa.gov/registry/sip.html)
- [Using SIP Coefficients (HNSky)](https://www.hnsky.org/sip.htm)
- [LSST FitSipDistortionTask](https://pipelines.lsst.io/modules/lsst.meas.astrom/tasks/lsst.meas.astrom.FitSipDistortion.html)
- [Astrometry.net SIP Implementation](https://github.com/dstndstn/astrometry.net/blob/main/util/sip.c)
- [PixInsight Distortion Correction](https://www.pixinsight.com/tutorials/sa-distortion/index.html)
- [LSST SIP Interpretation Discussion](https://community.lsst.org/t/interpretation-of-the-sip-standard-in-afw/295)
- [Thin Plate Spline (Wikipedia)](https://en.wikipedia.org/wiki/Thin_plate_spline)
- [Normal Equations Stability (GSL)](https://www.gnu.org/software/gsl/doc/html/lls.html)
- [Astrometry.net Order Selection Discussion](https://groups.google.com/g/astrometry/c/3y2HoRTNXN8)
