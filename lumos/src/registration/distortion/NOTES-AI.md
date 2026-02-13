# Distortion Module

## Architecture

Two distortion correction models for astronomical image registration:

1. **SIP Polynomial** (`sip/mod.rs`, 528 lines): Parametric polynomial distortion
   following the FITS WCS SIP convention (Shupe et al. 2005). Integrated into the
   registration pipeline via `WarpTransform`.
2. **Thin Plate Spline** (`tps/mod.rs`, 445 lines): Non-parametric radial basis
   function interpolation. Fully implemented and tested but not yet integrated
   into the pipeline (marked `#![allow(dead_code)]`).

### Pipeline Integration

```
registration/mod.rs:312-328  -->  SipPolynomial::fit_from_transform()
result.rs:172-177             -->  RegistrationResult::warp_transform() -> WarpTransform
transform.rs:346-352          -->  WarpTransform::apply(p) = transform.apply(sip.correct(p))
```

For each output pixel `p`, the source coordinate is:
- With SIP: `src = transform.apply(sip.correct(p))`
- Without SIP: `src = transform.apply(p)`

Config fields: `sip_enabled` (default false), `sip_order` (default 3),
`sip_reference_point` (default None = centroid). Presets `precise()`,
`wide_field()`, `high_star_count()` enable SIP by default.

---

## SIP Algorithm Description

### The SIP Standard (Shupe et al. 2005)

The SIP convention defines distortion as polynomials applied to pixel
coordinates relative to CRPIX, before the CD matrix:

```
u = pixel_x - CRPIX1
v = pixel_y - CRPIX2

U = u + Sum_{p+q>=2} A_pq * u^p * v^q     (forward, A/B polynomials)
V = v + Sum_{p+q>=2} B_pq * u^p * v^q

(xi, eta) = CD * (U, V)                     (intermediate world coords)
```

Linear terms (p+q < 2) are excluded because they are absorbed by the CD matrix.

Inverse polynomials (AP/BP) go the other direction: given intermediate world
coordinates after inverse-CD, recover pixel coordinates.

### This Implementation

**Fitting** (`sip/mod.rs:123-247`, `fit_from_transform`):

1. Validate order (2-5) and check point count >= 3 * terms.
2. Compute reference point (user-specified or centroid of ref_points).
3. Compute normalization scale = avg distance from ref_points to reference point.
4. Compute target residuals: `target_u[i] = -(transform(ref[i]).x - target[i].x) / norm_scale`.
   This is the correction needed so that `transform(ref + correction) ~ target`.
5. Build normal equations A^T*A and A^T*b from monomial basis functions.
6. Solve via Cholesky (with condition check) or LU fallback.
7. Iterative sigma-clipping: compute per-point residual magnitudes, reject
   outliers beyond `median + clip_sigma * MAD * 1.4826`, refit. Default: 3
   iterations, 3.0 sigma.

**Evaluation** (`sip/mod.rs:250-304`, `correct` and `correction_at`):

```
correction_at(p):
  u = (p.x - ref.x) / norm_scale
  v = (p.y - ref.y) / norm_scale
  du = Sum coeffs_u[j] * u^p * v^q    (for each term j)
  dv = Sum coeffs_v[j] * u^p * v^q
  return DVec2(du * norm_scale, dv * norm_scale)

correct(p) = p + correction_at(p)
```

### Term Counts by Order

| Order | Terms per axis | Total p+q pairs |
|-------|---------------|-----------------|
| 2     | 3             | (2,0), (1,1), (0,2) |
| 3     | 7             | + (3,0), (2,1), (1,2), (0,3) |
| 4     | 12            | + 5 more |
| 5     | 18            | + 6 more |

Formula: `(order+1)(order+2)/2 - 3`

---

## SIP Correctness Analysis

### Direction: CORRECT

The SIP standard defines A/B as the **forward** polynomial: pixel offsets
`(u, v)` from CRPIX are corrected before applying the CD matrix (or in our
case, the homography). The code at `sip/mod.rs:151-159` computes:

```rust
targets_u[i] = -(transform.apply(ref[i]).x - target[i].x) / norm_scale
```

This computes the correction `du` such that `transform(ref + du) ~ target`,
which is exactly the forward SIP polynomial. The `correct()` method at line
250 adds this correction to the input point before the linear transform is
applied.

`WarpTransform::apply` at `transform.rs:346-352` computes
`transform.apply(sip.correct(p))`, which is the standard SIP forward pipeline:
correct pixel coords, then apply linear transform. **This is correct.**

Matches: Astrometry.net, STWCS, Astropy, MOPEX.

### Coordinate System: CORRECT

Coordinates are defined as offsets from the reference point (`sip/mod.rs:177`,
`sip/mod.rs:292`):

```rust
let u = (ref_points[i].x - ref_pt.x) / norm_scale;
let v = (ref_points[i].y - ref_pt.y) / norm_scale;
```

This matches the SIP standard `u = pixel_x - CRPIX1`. The additional
normalization by `norm_scale` is a numerical improvement not in the standard
but algebraically equivalent (coefficients are in normalized space,
`correction_at` denormalizes the result at line 303).

### Normalization: GOOD

`sip/mod.rs:148`, `avg_distance()` at line 331: average Euclidean distance
from points to reference. This keeps monomial basis values near O(1),
reducing condition number of A^T*A. LSST uses a similar "ScaledPolynomialTransform"
approach. Astrometry.net uses raw pixel offsets (no normalization), which can
cause conditioning issues at order >= 4 on large images.

### Normal Equations Assembly: CORRECT

`build_normal_equations` at `sip/mod.rs:361-403`: correctly builds the
symmetric A^T*A and A^T*b. Exploits the fact that A^T*A is shared between
U and V axes (the design matrix depends only on point positions, not
targets). The symmetric fill `ata[k*n+j] = ata[j*n+k]` at line 393 is
correct.

### Polynomial Term Generation: CORRECT

`term_exponents` at `sip/mod.rs:313-322`: generates `(p, q)` pairs with
`2 <= p+q <= order`, correctly excluding linear terms. The iteration order
is `total` from 2 to `order`, then `p` from `total` down to 0. This
produces terms in the canonical SIP ordering.

For order 5: `(5+1)*(5+2)/2 - 3 = 18` terms. Verified by MAX_TERMS = 18.

### Solver: CORRECT with good fallback

**Cholesky** (`solve_cholesky`, line 408): Primary solver for the SPD normal
equations matrix A^T*A. O(n^3/6) operations for n up to 18.

**Condition number check** (line 432-443): After Cholesky factorization,
computes `max(diag(L)) / min(diag(L))`. If ratio exceeds 1e5 (implying
cond(A^T*A) ~ 1e10), falls back to LU. This is a practical and cheap
estimate. The threshold 1e5 is reasonable -- digits of accuracy lost is
approximately `log10(cond)`, so cond ~1e10 loses ~10 digits from the
available ~15 of f64.

**LU fallback** (`solve_lu`, line 471): Gaussian elimination with partial
pivoting. Correct implementation. Handles the ill-conditioned case where
Cholesky would produce unreliable results.

**Note**: Astrometry.net uses QR decomposition on the full design matrix A
(not the normal equations A^T*A), which avoids squaring the condition number.
However, for the small systems here (max 18 unknowns) with normalized
coordinates, the normal equations approach with condition monitoring is
adequate. QR would be an improvement but not critical.

### Sigma-Clipping: CORRECT

`sip/mod.rs:169-238`: MAD-based iterative outlier rejection.

Algorithm verification:
1. Initial fit on all points (line 164-166).
2. Per-point residual magnitudes in normalized space (lines 172-191).
3. Active-only median and MAD computation (lines 194-210).
4. `MAD_TO_SIGMA = 1.4826022` (line 212) -- the standard constant
   relating MAD to sigma for Gaussian distributions. The exact value
   is `1 / Phi^{-1}(3/4) = 1.4826...`. Correct.
5. Rejection threshold: `median + clip_sigma * mad * MAD_TO_SIGMA` (line 222).
   One-sided rejection (only above median + threshold). This is appropriate
   because residual magnitudes are non-negative, so only the upper tail
   contains outliers.
6. Early termination when no points rejected or when too few points remain
   (lines 200-201, 228-229).
7. Re-solve with surviving points (lines 233-237).

This matches LSST practice (3 iterations, 3.0 sigma, MAD-based).

### Minimum Point Count: CORRECT

`sip/mod.rs:140`: `n < 3 * terms.len()` returns None. This matches the
Astrometry.net community recommendation of ~3x the number of polynomial
terms. For order 5 (18 terms), need >= 54 points.

---

## TPS Analysis

### Kernel Function: CORRECT

`tps/mod.rs:276-278`: `U(r) = r^2 * ln(r)` with `U(0) = 0`.

This is the standard 2D thin-plate spline kernel. The mathematical limit
`lim_{r->0} r^2 ln(r) = 0` is correct. The threshold `r < 1e-10` is
appropriate (avoids `0 * -inf = NaN`). Matches Bookstein (1989) and the
Wikipedia reference.

### System Assembly: CORRECT

`tps/mod.rs:96-129`: Standard TPS formulation:

```
[K + lambda*I  P] [w]   [v]
[P^T           0] [a] = [0]
```

where `K[i,j] = U(||src_i - src_j||)`, `P[i,:] = [1, x_i, y_i]`.
The lower-right 3x3 block is zeros. Both x and y directions share the
same system matrix but have different RHS vectors (lines 134-139).

### Coordinate Normalization: CORRECT

`compute_normalization` at `tps/mod.rs:257-270`: bounding-box center and
half-extent scaling to `[-1, 1]`. Both source and target points are
normalized with the same parameters (lines 85-92). `transform()` normalizes
input and denormalizes output (lines 173, 190).

This is critical because `r^2 * ln(r)` amplifies scale differences
catastrophically -- for raw pixel coords with r~7200, kernel values reach
~4.6e8, while affine terms are O(6000).

### Regularization: CORRECT

`TpsConfig.regularization` (default 0.0) adds `lambda * I` to the K block
diagonal (line 112). For exact interpolation, lambda=0. For smoothing with
noisy data, lambda > 0 trades interpolation exactness for smoothness.

PixInsight defaults to lambda=0.25 for astrometry.

### Solver: CORRECT but suboptimal allocation

`solve_linear_system` at `tps/mod.rs:282-341`: LU decomposition with partial
pivoting using `Vec<Vec<f64>>`. Correct algorithm choice (the TPS system
matrix is symmetric but not positive definite due to the zero block, so
Cholesky cannot be used).

### Bending Energy: CORRECT

`bending_energy` at `tps/mod.rs:208-224`: `E = Sum_i Sum_j w_i * w_j * U(||c_i - c_j||)`.
This is the standard formula. For affine transformations (translation,
rotation, scaling), all weights should be zero and energy should be zero.
Tests verify this at `tps/tests.rs:740-782`.

### DistortionMap: CORRECT

`DistortionMap` at `tps/mod.rs:343-442`: grid-sampled distortion vectors
with bilinear interpolation. This is the standard approach for efficient
TPS evaluation during warping (O(1) per pixel instead of O(N)).

### Integration Status

Fully implemented and tested (23 tests) but marked `#![allow(dead_code)]`
at `tps/mod.rs:2`. `tps_kernel` is exported as `pub(crate)` but unused by
other crate code. `DistortionMap` is exported but unused.

---

## Detailed Comparison with FITS SIP Standard

| Aspect | SIP Standard (Shupe 2005) | This Implementation | Assessment |
|--------|--------------------------|---------------------|------------|
| Polynomial direction | Forward (A/B): pixel -> corrected pixel | Forward: ref -> corrected ref | CORRECT |
| Reference point origin | CRPIX (mandatory) | Configurable, default=centroid | Functional but not FITS-compatible by default |
| Coordinate definition | `u = pixel - CRPIX` | `u = (pixel - ref) / norm_scale` | Algebraically equivalent (normalized) |
| Linear terms | Excluded (p+q < 2) | Excluded (p+q < 2) | CORRECT |
| Polynomial order range | 2-9 (spec, typically 2-4) | 2-5 | Adequate (order > 5 rarely used) |
| Coefficient storage | FITS keywords A_p_q, B_p_q | Rust ArrayVec | Internal only, no FITS I/O |
| Normalization | Not specified | avg-distance normalization | Improvement over standard |
| Solver | Not specified (Astrometry.net: QR) | Cholesky + LU fallback | Adequate with condition monitoring |
| Outlier rejection | Not specified (LSST: sigma-clip) | MAD-based sigma-clipping | Matches LSST best practice |
| Inverse poly (AP/BP) | Optional, grid-sampled fit | Not implemented | Gap for FITS export |
| FITS header I/O | A_ORDER, A_i_j, B_ORDER, B_i_j, AP_*, BP_* | Not implemented | Gap for interoperability |
| CTYPE suffix | `-SIP` in RA---TAN-SIP, DEC--TAN-SIP | N/A (internal use) | Not needed for registration |
| A_DMAX, B_DMAX | Max correction magnitude | `max_correction()` method | Equivalent |

### Direction Correctness (detailed)

The SIP standard pipeline is:

```
pixel -> subtract CRPIX -> apply A/B polynomial -> multiply by CD -> (xi, eta)
```

Our pipeline is:

```
output_pixel -> sip.correct(p) -> transform.apply() -> source_pixel
```

where `sip.correct(p) = p + correction_at(p)` adds the polynomial correction
to the pixel position before the linear transform. This is the forward (A/B)
direction. The fitting at `sip/mod.rs:151-159` computes residuals as the
negative of `transform(ref) - target`, which is the correction needed to make
`transform(ref + correction) = target`. Correct.

### Reference Point Subtlety

The SIP standard mandates CRPIX as the reference point. Our implementation
defaults to the centroid of input points when `reference_point` is None. This
is fine for internal registration but produces coefficients that are not
directly FITS-exportable. For FITS interoperability, callers should set
`sip_reference_point = Some(image_center)`.

The test `test_reference_point_crpix_vs_centroid` (`sip/tests.rs:300-370`)
demonstrates that using the correct optical center produces better fits for
radially symmetric distortion patterns.

---

## Issues Found

### Issue 1: Stale README.md files (documentation)

**`distortion/README.md:23`** references methods that do not exist in the code:
`compute_inverse`, `inverse_correct`, `inverse_correction_at`,
`inverse_correct_points`, `has_inverse`. It also claims "30 tests" in
sip/tests.rs (actual: 15) and "22 tests" in tps/tests.rs (actual: 23).

**`distortion/sip/README.md:44`** states "Outlier rejection: None" in the
comparison table, but sigma-clipping is now implemented. Line 54 references
`fit_residuals` method that does not exist.

**Fix**: Update both README.md files to match current code, or delete them
since NOTES-AI.md covers the same content.

### Issue 2: No inverse polynomial (AP/BP) support

The SIP standard defines inverse polynomials AP/BP for sky-to-pixel mapping.
These are separate fits (not exact mathematical inverses), typically computed
by grid-sampling the forward correction and fitting an inverse polynomial.

**Impact**: Cannot export SIP coefficients to FITS headers for consumption by
DS9, Astropy, SAOImage, etc. Not needed for the current registration-only
pipeline but blocks future FITS interoperability.

**Implementation approach**: Grid-sample `correction_at()` on a dense grid
(100x100), then fit inverse polynomial using the same normal equations solver.
LSST uses AP_ORDER = A_ORDER + 1 (capped).

### Issue 3: TPS solver uses Vec<Vec<f64>> (minor performance)

`tps/mod.rs:289-297`: The `solve_linear_system` function builds the augmented
matrix as `Vec<Vec<f64>>`, creating N+4 separate heap allocations. A flat
`Vec<f64>` with `row * (n+1)` stride indexing (as used in the SIP LU solver
`solve_lu` at `sip/mod.rs:471-527`) would be a single allocation.

**Impact**: Negligible for typical N < 200 control points. Only matters if TPS
is integrated into the pipeline with many control points.

### Issue 4: TPS not integrated into pipeline

The TPS module is fully implemented and tested (23 tests) but has no code path
that uses it (`#![allow(dead_code)]`). PixInsight uses TPS as its primary
distortion model. Integration would provide an alternative for cases where SIP
polynomial residuals remain high.

### Issue 5: No fit quality diagnostics returned

`SipPolynomial::fit_from_transform` returns `Option<Self>` but provides no
information about:
- RMS/max residual after fitting
- Number of points rejected by sigma-clipping
- Condition number of the normal equations matrix
- Whether the LU fallback was triggered

This makes it difficult for callers to assess fit quality or decide whether to
increase/decrease polynomial order.

---

## Prioritized Improvements

### Priority 1 (documentation bugs)

1. Fix stale `distortion/README.md` -- remove references to nonexistent
   inverse methods, update test counts.
2. Fix stale `sip/README.md` -- update comparison table (sigma-clipping
   exists), remove `fit_residuals` reference, update method list.

### Priority 2 (useful features)

3. Add fit quality return type: `SipFitResult { polynomial, rms_residual,
   max_residual, points_rejected, lu_fallback_used }` instead of
   `Option<SipPolynomial>`.
4. Consider returning condition number estimate from the solver.

### Priority 3 (future interoperability)

5. Implement inverse polynomial (AP/BP) fitting via grid sampling for
   future FITS header export.
6. Implement FITS header I/O (A_ORDER, A_i_j keywords) if FITS export is
   needed.

### Priority 4 (integration)

7. Integrate TPS into the registration pipeline as an alternative to SIP
   for wide-field or complex distortion cases.
8. Flatten TPS solver allocation (`Vec<Vec<f64>>` -> flat `Vec<f64>`).

### Not needed

- **Chebyshev/Legendre basis**: SIP standard requires power-basis polynomials.
  Existing normalization handles conditioning adequately for orders 2-5.
- **Tikhonov regularization for SIP**: Overfitting is addressed by 3x point
  count requirement and sigma-clipping.
- **QR decomposition**: Would avoid squaring the condition number, but for
  max 18 unknowns with normalized coords, Cholesky+LU is sufficient.
- **Brown-Conrady model**: SIP order 3 subsumes it.
- **TPV convention**: Different use case (sky-space corrections). SIP is
  sufficient for pixel-space registration.

---

## SIP vs TPS: When to Use Which

| Aspect | SIP Polynomial | Thin Plate Spline |
|--------|---------------|-------------------|
| Model type | Global parametric | Local non-parametric |
| Best for | Optical distortion (radial) | Complex, non-uniform distortion |
| Parameters | 3-18 per axis (order 2-5) | N+3 per axis (N = control points) |
| Overfitting risk | High at order >= 4 | Low with regularization |
| Extrapolation | Diverges outside data hull | Extrapolates linearly (affine) |
| FITS standard | Yes (SIP keywords) | No |
| Speed (evaluate) | O(terms) ~ns | O(N) ~us, or O(1) with DistortionMap |
| Industry use | Astrometry.net, Siril, HST | PixInsight, medical imaging |

**Default**: SIP order 3 (handles barrel/pincushion + mustache distortion).
Use TPS for wide-field mosaics with complex distortion, or when SIP residuals
remain high. Astrometry.net defaults to SIP order 2; PixInsight defaults to TPS.

---

## File Structure

```
distortion/
  mod.rs           Re-exports: SipConfig, SipPolynomial, TpsConfig,
                   ThinPlateSpline, DistortionMap, tps_kernel (pub(crate))
  NOTES-AI.md      This file
  README.md        Human-readable overview (STALE -- references nonexistent methods)
  sip/
    mod.rs         SipPolynomial, SipConfig, solvers, helpers (528 lines)
    README.md      SIP standard overview (STALE -- see Issue 1)
    tests.rs       15 tests: barrel, pincushion, orders, edge cases, CRPIX,
                   sigma-clipping (4 tests), ill-conditioned LU fallback
  tps/
    mod.rs         ThinPlateSpline, TpsConfig, DistortionMap, tps_kernel,
                   solve_linear_system (445 lines)
    tests.rs       23 tests: exact interpolation, transforms (translation,
                   scaling, rotation, identity), barrel distortion,
                   regularization, edge cases (collinear, clustered, large
                   deformation, extreme coordinates), DistortionMap
```

## References

### SIP Standard
- [SIP Convention v1.0 (Shupe et al. 2005)](https://irsa.ipac.caltech.edu/data/SPITZER/docs/files/spitzer/shupeADASS.pdf) -- the defining paper
- [SIP FITS Registry](https://fits.gsfc.nasa.gov/registry/sip.html) -- official FITS registration
- [SIP Convention (STWCS docs)](https://stwcs.readthedocs.io/en/latest/fits_convention_tsr/source/sip.html) -- HST implementation
- [Astropy SIP Note](https://docs.astropy.org/en/stable/wcs/note_sip.html) -- implementation pitfalls
- [Using SIP Coefficients (HNSky)](https://www.hnsky.org/sip.htm) -- clear formula reference

### Industry Implementations
- [Astrometry.net SIP Implementation (fit-wcs.c)](https://github.com/dstndstn/astrometry.net/blob/main/util/fit-wcs.c) -- QR solver, no normalization
- [Astrometry.net Order Selection Discussion](https://groups.google.com/g/astrometry/c/3y2HoRTNXN8) -- practical order limits
- [LSST FitSipDistortionTask](https://pipelines.lsst.io/modules/lsst.meas.astrom/tasks/lsst.meas.astrom.FitSipDistortion.html) -- sigma-clipping, scaled polynomial
- [PixInsight Distortion Correction](https://www.pixinsight.com/tutorials/sa-distortion/index.html) -- TPS approach
- [PixInsight Solver Distortion Algorithm](https://pixinsight.com/tutorials/solver-distortion/) -- surface simplifiers

### Related Standards
- [TPV WCS Convention](https://fits.gsfc.nasa.gov/registry/tpvwcs/tpv.html) -- PV polynomials (SExtractor/SWarp)
- [SIP to PV Conversion (Shupe et al. 2012)](https://web.ipac.caltech.edu/staff/shupe/reprints/SIP_to_PV_SPIE2012.pdf)
- [FITS WCS Paper IV Draft (Calabretta & Greisen)](https://fits.gsfc.nasa.gov/wcs/dcs_20040422.pdf)

### Numerical Methods
- [Normal Equations Stability (GSL)](https://www.gnu.org/software/gsl/doc/html/lls.html) -- condition number squaring
- [Condition Number (Wikipedia)](https://en.wikipedia.org/wiki/Condition_number)
- [Normal Equations (Driscoll)](https://tobydriscoll.net/fnc-julia/leastsq/normaleqns.html) -- kappa(A^T A) = kappa(A)^2

### Thin Plate Splines
- [Thin Plate Spline (Wikipedia)](https://en.wikipedia.org/wiki/Thin_plate_spline) -- mathematical reference
- [TPS Geometric Tools (Eberly)](https://www.geometrictools.com/Documentation/ThinPlateSplines.pdf) -- derivation
- [PixInsight SurfaceSpline API](https://pixinsight.com/developer/pcl/doc/html/classpcl_1_1SurfaceSpline.html) -- PCL implementation
