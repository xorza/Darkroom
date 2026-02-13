# Distortion Module - Research Analysis

## Overview

Two distortion correction models for astronomical image registration:

1. **SIP Polynomial** (`sip/mod.rs`): Parametric polynomial distortion following
   the FITS WCS SIP convention (Shupe et al. 2005). Integrated into the
   registration pipeline via `WarpTransform`.
2. **Thin Plate Spline** (`tps/mod.rs`): Non-parametric radial basis function
   interpolation. Fully implemented and tested but not yet integrated into the
   pipeline (marked `dead_code`).

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
(transform.rs) computes `src = transform.apply(sip.correct(p))` where
`p` is the output pixel. This means SIP correction is applied to the output
(reference) frame pixel position before the linear transform maps it to the
source frame. This is the correct direction.

**Verdict**: Direction is correct. Matches Astrometry.net, STWCS, MOPEX usage.

### Reference Point Handling

`sip/mod.rs:142-145`: When `reference_point` is None, the centroid of
`ref_points` is used. When specified, it acts as CRPIX.

The SIP standard mandates coordinates be relative to CRPIX. The pipeline at
`registration/mod.rs:319-322` passes `config.sip_reference_point` through
to `SipConfig`, which defaults to None (centroid). Users can set
`Config.sip_reference_point = Some(DVec2::new(w/2, h/2))` for proper
optical distortion modeling.

Test `test_reference_point_crpix_vs_centroid` (sip/tests.rs:300-370)
demonstrates that image-center reference produces better fits when distortion
is radial from the optical axis.

### Coordinate Normalization

`sip/mod.rs:146`: Coordinates are normalized by average distance from points
to reference point. This is good practice (reduces condition number of normal
equations). The LSST pipeline uses similar "scaled polynomial transform"
approach.

This is an improvement over the SIP standard, which does not specify
normalization. Astrometry.net uses raw pixel offsets, which can cause
conditioning issues at order >= 4.

### Normal Equation Assembly

`sip/mod.rs:359-401`: Correctly builds A^T*A and A^T*b. Exploits shared A^T*A
matrix for both U and V axes (the design matrix depends only on point positions,
not targets).

### Polynomial Term Generation

`sip/mod.rs:311-320`: Generates monomials with `2 <= p+q <= order`, correctly
excluding linear terms. Term count: `(order+1)(order+2)/2 - 3`. For order 5:
`6*7/2 - 3 = 18`. Correct.

### Monomial Evaluation

`sip/mod.rs:324-326`: Uses `u.powi(p) * v.powi(q)`. This is numerically
adequate for the small orders (2-5) and normalized coordinates used here.
A Horner-scheme evaluation could reduce operations for individual polynomial
evaluation, but the gain is negligible since:
- The terms are evaluated independently (not nested in one polynomial)
- Coordinates are normalized to ~O(1) by avg-distance scaling
- The powi() call is well-optimized for small integer exponents

### Solver Choice

**Cholesky** (`sip/mod.rs:406-451`): Primary solver for SPD normal equations.
O(n^3/3) operations.

**LU fallback** (`sip/mod.rs:455-511`): Gaussian elimination with partial
pivoting. Triggered when Cholesky encounters non-positive-definite matrix.

**Missing**: No condition number check. A cheap estimate during Cholesky
(ratio of largest to smallest diagonal of L) could detect ill-conditioning.

### Sigma-Clipping

`sip/mod.rs:167-236`: Iterative outlier rejection is **implemented and active**.
Uses MAD-based sigma estimation with configurable `clip_sigma` (default 3.0)
and `clip_iterations` (default 3). The pipeline uses default config, so
sigma-clipping runs by default when SIP is enabled.

Algorithm:
1. Fit polynomial on all points
2. Compute per-point residual magnitudes
3. Estimate sigma via MAD (median absolute deviation) with MAD_TO_SIGMA = 1.4826
4. Reject points beyond `median + clip_sigma * MAD * MAD_TO_SIGMA`
5. Refit with surviving points
6. Repeat for `clip_iterations` rounds

This matches LSST practice. Tests in sip/tests.rs verify outlier rejection
and early convergence on clean data.

### max_correction() Method

`sip/mod.rs:273-286`: Samples a grid and returns maximum correction magnitude.
Useful for sanity-checking (detecting overfitting if max_correction is
unreasonably large). Could be improved by also returning the location of max.

---

## TPS Correctness Analysis

### Kernel Function

`tps/mod.rs:229-231`: `U(r) = r^2 * ln(r)` with `U(0) = 0`.

Standard 2D thin-plate spline kernel. The limit `lim_{r->0} r^2 ln(r) = 0`
is correct and handled by `r < 1e-10` check. Matches Bookstein (1989).

### System Assembly

`tps/mod.rs:85-113`: Correct standard TPS formulation:
```
[K + lambda*I  P] [w]   [v]
[P^T           0] [a] = [0]
```

### Regularization

`TpsConfig.regularization` (default 0.0) controls smoothing. PixInsight
defaults to 0.25 because star positions have measurement errors. For
registration with imperfect star detections, regularization > 0 is
recommended.

### Solver

`tps/mod.rs:234-293`: LU decomposition with partial pivoting. Correct choice
(the TPS matrix is not SPD). Uses `Vec<Vec<f64>>` which is allocation-heavy
but correct.

### Coordinate Normalization — FIXED

Coordinates are now centered and scaled to `[-1, 1]` before building the TPS
system matrix. `compute_normalization()` computes bounding-box center and
half-extent; both source and target points are normalized with the same
parameters. `transform()` normalizes input and denormalizes output.

Result: `test_tps_large_coordinates` (offset=10000) tolerance tightened from
1.0 to 1e-3 pixel. New `test_tps_extreme_coordinates` (offset=100000) passes
with residuals < 1e-5.

### Integration Status

Fully implemented and tested (16 tests) but marked `dead_code` because no
public-facing code path uses it. `tps_kernel` exported as `pub(crate)` but
unused by other crate code.

### Performance Characteristics

- **Solve**: O(n^3) for (n+3) x (n+3) system. Negligible for n<200.
- **Evaluate**: O(n) per point (sum over all control points). Expensive for
  warping. Standard approach: pre-compute `DistortionMap` on grid, then
  bilinear-interpolate during warp (O(1) per pixel). PixInsight does this.

---

## Industry Comparison

### SIP Convention (Shupe et al. 2005)

| Aspect | SIP Standard | This Implementation |
|--------|-------------|---------------------|
| CTYPE suffix | RA---TAN-SIP / DEC--TAN-SIP | Not applicable (internal use) |
| Reference point | CRPIX (mandatory) | Configurable (default: centroid) |
| Linear terms | Excluded (in CD matrix) | Excluded (in homography) |
| Polynomial order | 2-9 (spec), typically 2-4 | 2-5 |
| Normalization | Not specified | avg-distance (improvement) |
| Inverse (AP/BP) | Optional, grid-sampled | Not implemented |
| Sigma-clipping | Not specified | Implemented (MAD-based, 3 iterations) |
| FITS I/O | Defined (A_ORDER, A_p_q keywords) | Not implemented |

### TPV Convention (TAN + PV polynomials)

The TPV convention uses PV keywords (PV1_0 through PV1_39) for polynomial
corrections in intermediate world coordinates (xi, eta), supporting up to
order 7 with 40 coefficient pairs. Key differences from SIP:
- SIP corrects in pixel space; TPV corrects in sky space
- TPV includes radial terms (r, r^3, r^5, r^7)
- SIP is more common in space-based data; TPV in ground-based (SExtractor, SCAMP)

Not implemented in this module. SIP-to-TPV conversion is possible (Shupe et
al. 2012) but not needed for the current registration-only use case.

### OpenCV Brown-Conrady Model

OpenCV uses: `x' = x(1 + k1*r^2 + k2*r^4 + k3*r^6)` (radial) plus
`2*p1*x*y + p2*(r^2 + 2*x^2)` (tangential). Five parameters: (k1, k2, p1, p2, k3).

This is simpler than SIP (fewer parameters, purely radial + tangential terms)
but does not capture the full range of distortions that a general 2D polynomial
can. The SIP model with order 3 subsumes Brown-Conrady: barrel/pincushion
(k1*r^2 ~ order 3 SIP terms) plus tangential (xy cross-terms).

Not needed as a separate model.

### Astrometry.net

- Default SIP order: 2 (quadratic)
- Uses QR decomposition (more stable than normal equations for edge cases)
- Uses `--crpix-center` to center distortion at image center
- No sigma-clipping during SIP fitting (relies on match quality)
- Warns: higher orders "may produce totally bogus results" outside convex hull

Key insight from Astrometry.net community: minimum matched stars for order N
is `(N+1)(N+2)/2` but practical minimum is ~3x that. For order 4 (15 terms),
need ~45 matched stars.

### LSST FitSipDistortionTask

- Uses sigma-clipping (3 iterations, 3-sigma threshold)
- Uses scaled polynomial intermediate representation (similar to our normalization)
- Fits reverse (AP/BP) first, then derives forward from grid (more numerically stable)
- Uses intrinsic scatter estimation

### PixInsight

- Uses TPS (thin plate splines) exclusively for distortion correction
- Default regularization (smoothness): 0.25
- Iterative successive approximations algorithm (up to 100 iterations)
- Pre-computes distortion grid for efficient warping
- "Surface simplifiers" for unprecedented astrometric accuracy

---

## Issues and Recommendations

### Resolved Issues (previously identified, now fixed)

1. **Sigma-clipping**: Now implemented in `fit_from_transform` with MAD-based
   rejection. Default 3.0 sigma, 3 iterations.

### Current Issues

#### Important

1. ~~**TPS lacks coordinate normalization**~~ — FIXED. Coordinates now centered
   and scaled to `[-1, 1]` in `fit()`, with normalize/denormalize in `transform()`.

2. **No condition number monitoring** in Cholesky solver (`sip/mod.rs:406-451`).
   Compute `max(diag(L)) / min(diag(L))` during factorization and log a warning
   if it exceeds ~1e10.

3. ~~**No polynomial order validation against point count**~~ — FIXED.
   Now requires `n >= 3 * terms.len()` (Astrometry.net practice). Returns
   `None` if insufficient points. Order 5 (18 terms) needs ≥54 points.

4. **Stale README.md** (`sip/README.md`). References `compute_inverse`,
   `inverse_correct`, `inverse_correction_at`, `inverse_correct_points`, and
   `has_inverse` methods that do not exist in the code. Either implement
   inverse polynomials or remove references.

#### Minor

5. **TPS solver uses Vec<Vec<f64>>** (`tps/mod.rs:242-250`). Each row is a
   separate heap allocation. A flat `Vec<f64>` with `row * (n+1)` indexing
   (as used in the SIP LU solver) would be more efficient.

6. **TPS kernel evaluates ln() per call** (`tps/mod.rs:229-231`). For warping,
   pre-compute a `DistortionMap` grid and bilinear-interpolate (O(1) per pixel).

7. **No inverse polynomial (AP/BP) support**. The SIP standard defines AP/BP
   for sky-to-pixel mapping. Needed for FITS header export. Implementation:
   grid-sample the forward correction, fit inverse polynomial via same solver.

### Not Needed

- **Chebyshev/Legendre basis**: SIP standard requires power-basis polynomials.
  Internal use of orthogonal polynomials would require conversion for FITS
  export. Marginal gain with existing normalization at orders 2-5.
- **Tikhonov regularization for SIP**: Not needed with sigma-clipping and
  coordinate normalization. Overfitting is addressed by order validation.
- **Brown-Conrady model**: SIP order 3 subsumes it. Extra model adds complexity
  without benefit.
- **TPV convention**: Different use case (sky-space corrections). SIP is
  sufficient for the registration pipeline's pixel-space corrections.

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

**Default**: SIP order 3 (handles barrel/pincushion). Use TPS for wide-field
mosaics with complex distortion, or when SIP residuals remain high.
Astrometry.net defaults to SIP order 2; PixInsight defaults to TPS.

---

## File Structure

```
distortion/
  mod.rs           Re-exports: SipConfig, SipPolynomial, TpsConfig,
                   ThinPlateSpline, DistortionMap, tps_kernel
  NOTES-AI.md      This file
  README.md        Human-readable overview (partially stale)
  sip/
    mod.rs         SipPolynomial fitting + evaluation (512 lines)
    README.md      SIP standard overview (partially stale -- references missing methods)
    tests.rs       11 tests: barrel, pincushion, orders, edge cases, CRPIX,
                   sigma-clipping (3 tests)
  tps/
    mod.rs         ThinPlateSpline, DistortionMap, tps_kernel (398 lines)
    tests.rs       16 tests: interpolation, transforms, regularization, edge cases
```

## References

- [SIP Convention (Shupe et al. 2005)](https://irsa.ipac.caltech.edu/data/SPITZER/docs/files/spitzer/shupeADASS.pdf)
- [SIP FITS Registry](https://fits.gsfc.nasa.gov/registry/sip.html)
- [SIP Convention (STWCS docs)](https://stwcs.readthedocs.io/en/latest/fits_convention_tsr/source/sip.html)
- [Astropy SIP Note](https://docs.astropy.org/en/stable/wcs/note_sip.html)
- [Using SIP Coefficients (HNSky)](https://www.hnsky.org/sip.htm)
- [TPV WCS Convention](https://fits.gsfc.nasa.gov/registry/tpvwcs/tpv.html)
- [SIP to PV Conversion (Shupe et al. 2012)](https://web.ipac.caltech.edu/staff/shupe/reprints/SIP_to_PV_SPIE2012.pdf)
- [LSST FitSipDistortionTask](https://pipelines.lsst.io/modules/lsst.meas.astrom/tasks/lsst.meas.astrom.FitSipDistortion.html)
- [Astrometry.net SIP Implementation](https://github.com/dstndstn/astrometry.net/blob/main/util/sip.c)
- [Astrometry.net Order Selection Discussion](https://groups.google.com/g/astrometry/c/3y2HoRTNXN8)
- [PixInsight Distortion Correction](https://www.pixinsight.com/tutorials/sa-distortion/index.html)
- [PixInsight Solver Distortion Algorithm](https://pixinsight.com/tutorials/solver-distortion/)
- [FITS WCS Paper IV Draft (Calabretta & Greisen)](https://fits.gsfc.nasa.gov/wcs/dcs_20040422.pdf)
- [OpenCV Camera Calibration](https://docs.opencv.org/4.x/dc/dbb/tutorial_py_calibration.html)
- [Brown-Conrady Model (Tangram Vision)](https://www.tangramvision.com/blog/the-innovative-brown-conrady-model)
- [Thin Plate Spline (Wikipedia)](https://en.wikipedia.org/wiki/Thin_plate_spline)
- [ALGLIB TPS Interpolation](https://www.alglib.net/thin-plate-spline-interpolation-and-fitting/)
- [Normal Equations Stability (GSL)](https://www.gnu.org/software/gsl/doc/html/lls.html)
- [Horner's Method Stability](https://en.wikipedia.org/wiki/Horner's_method)
