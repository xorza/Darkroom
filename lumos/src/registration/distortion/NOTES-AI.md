# Distortion Module

## Architecture

Two distortion correction models for astronomical image registration:

1. **SIP Polynomial** (`sip/mod.rs`, ~580 lines): Parametric polynomial distortion
   following the FITS WCS SIP convention (Shupe et al. 2005). Integrated into the
   registration pipeline via `WarpTransform`.
2. **Thin Plate Spline** (`tps/mod.rs`, ~445 lines): Non-parametric radial basis
   function interpolation. Fully implemented and tested but not yet integrated
   into the pipeline (marked `#![allow(dead_code)]`).

### Pipeline Integration

```
registration/mod.rs  -->  SipPolynomial::fit_from_transform() -> Option<SipFitResult>
                          result.sip_correction = sip_fit.polynomial.clone()
                          result.sip_fit = sip_fit
result.rs            -->  RegistrationResult::warp_transform() -> WarpTransform
transform.rs         -->  WarpTransform::apply(p) = transform.apply(sip.correct(p))
```

For each output pixel `p`, the source coordinate is:
- With SIP: `src = transform.apply(sip.correct(p))`
- Without SIP: `src = transform.apply(p)`

Config fields: `sip_enabled` (default false), `sip_order` (default 3),
`sip_reference_point` (default None = centroid). Presets `precise()`,
`wide_field()`, `high_star_count()` enable SIP by default.

---

## SIP: Implementation vs Industry

### Correctness Verification (All CORRECT)

| Component | Verification | Notes |
|-----------|-------------|-------|
| Polynomial direction | Forward (A/B): pixel->corrected, before linear transform | Matches Astrometry.net, Siril v1.3+, STWCS, MOPEX |
| Linear term exclusion | `2 <= p+q <= order` | Matches standard: linear terms absorbed by CD/homography |
| Term generation | `term_exponents()` produces canonical ordering | `(order+1)(order+2)/2 - 3` terms verified |
| Coordinate normalization | `avg_distance()` scaling | Better than Astrometry.net (none), similar to LSST (ScaledPolynomialTransform) |
| Normal equations | Shared A^T*A for both axes, symmetric fill | Correct: design matrix depends only on positions |
| Cholesky solver | SPD solver with condition estimate `max(diag(L))/min(diag(L))` | Threshold 1e5 implies cond(A^T*A) ~1e10, ~10 digits lost |
| LU fallback | Partial pivoting, triggered by non-PD or poor conditioning | Correct Gaussian elimination implementation |
| Sigma-clipping | MAD-based, `MAD_TO_SIGMA = 1.4826022`, one-sided rejection | Matches LSST practice (3 iter, 3 sigma, MAD-based) |
| Minimum points | `3 * terms.len()` | More conservative than Astrometry.net (1x), good overfitting prevention |
| Fit diagnostics | `SipFitResult` with RMS, max residual, point counts, max correction | Complete quality reporting |

### Comparison with Major Implementations

| Aspect | Astrometry.net | LSST | Siril | Ours |
|--------|---------------|------|-------|------|
| Solver | QR on design matrix | Scaled polynomial + affine | Unknown | Cholesky + LU on normal equations |
| Normalization | None (raw pixels) | Scaled polynomial transform | Unknown | avg-distance normalization |
| Outlier rejection | None | Sigma-clip (1 reject/iter max) | Unknown | MAD sigma-clip (all beyond threshold) |
| Default order | 2 | 4 | 3 (cubic) | 3 |
| Min points | 1x terms | Unknown | Unknown | 3x terms |
| Reference point | CRPIX (fixed) | Match centroid | CRPIX | Configurable (default centroid) |
| Inverse (AP/BP) | Grid-sample + QR | Grid-sample (100x100) | Unknown | Not implemented |
| Linear terms in basis | Yes (then extracted to CD) | Unknown | Unknown | Excluded from basis |

### Order Selection Guide

| Order | Terms | Min Stars (3x) | Distortion Types | Recommended For |
|-------|-------|----------------|------------------|-----------------|
| 2 | 3 | 9 | Simple quadratic | Short FL, few stars |
| 3 | 7 | 21 | Barrel/pincushion + mustache | **Default** (matches Siril) |
| 4 | 12 | 36 | Higher-order | Wide-field, many stars |
| 5 | 18 | 54 | HST-level complex | Space telescopes, 50+ stars |

Barrel distortion residuals `dx = k*u*r^2` are **cubic** (order 3), not quadratic.
Order 2 cannot capture barrel/pincushion distortion.

### Normal Equations vs QR

Theory says normal equations square the condition number: `cond(A^T*A) = cond(A)^2`.
With our avg-distance normalization, the design matrix condition for order 5 is
typically 1e3-1e5, so normal equations condition is 1e6-1e10. Our condition
monitoring (threshold 1e5 on L diagonal ratio) catches dangerous cases. QR would
avoid squaring entirely and is equally fast for n<=18, but is not critical given
our normalization. Astrometry.net's QR approach is slightly more robust but their
lack of normalization often makes it necessary.

### LSST's Different Strategy

LSST fits the **reverse** transform (AP/BP) first from star matches, then
grid-samples a 100x100 grid (+50px border) to fit the forward (A/B). This is more
robust because pixel-space measurement uncertainties are better understood. Their
sigma-clipping is very conservative: max 1 rejection per iteration. Our approach
(forward-direct, reject all outliers beyond threshold) is simpler and adequate for
registration.

### Subtle Design Choices

**Residual space assumption**: The fit computes correction targets as
`(target - transform(ref)) / norm_scale`, which is the residual in **output** space.
The exact approach would require `transform_inverse(target) - ref` (residual in
**input** space). For transforms near identity (small rotation, scale ~1.0), the
Jacobian is approximately I and the difference is second-order small. For typical
astronomical registration (rotation <5 deg, scale 0.99-1.01), this approximation
introduces negligible error. For large homographies (perspective >1%), the error
could become noticeable -- but such cases are rare in stacking. Not a bug, but
worth understanding.

**One-sided sigma clipping**: Residuals are magnitudes (always >= 0), so only
the right tail is clipped (`residual > median + threshold`). This is correct
for distance-based residuals, matching Astropy's `sigma_clip` behavior for
positive-definite quantities.

---

## TPS: Implementation vs Industry

### Correctness Verification (All CORRECT)

| Component | Verification | Notes |
|-----------|-------------|-------|
| Kernel U(r) = r^2 ln(r) | Standard 2D biharmonic Green's function | Matches Bookstein (1989), PixInsight, ALGLIB, Wolfram MathWorld |
| U(0) = 0 threshold | `r < 1e-10` returns 0 | Correct limit; prevents `0 * -inf = NaN` |
| System matrix | `[K+lambda*I  P; P^T  0]` | Standard TPS formulation (Wikipedia, Eberly) |
| Regularization | lambda on K diagonal only, zero block preserved | Correct: lambda=0 exact interp, lambda>0 smoothing |
| Normalization | Bounding-box center + half-extent to [-1,1] | Adequate; PixInsight uses mean-centered instead |
| Solver | LU with partial pivoting | Correct for symmetric indefinite systems |
| Bending energy | `E = wx^T*K*wx + wy^T*K*wy` | Standard formula (Bookstein 1989), `i!=j` skip is correct |
| DistortionMap | Grid-sampled + bilinear interpolation | Standard approach for O(1) per-pixel evaluation |

### Comparison with PixInsight (Primary Industry TPS User)

| Aspect | PixInsight | Ours |
|--------|-----------|------|
| Kernel | TPS (default), also Gaussian, Multiquadric, VariableOrder | TPS only |
| Solver | Bunch-Kaufman (LDLT) for symmetric indefinite | LU with partial pivoting |
| Normalization | Mean-centered, scaled by 1/max-radius | Bounding-box center, half-extent |
| Default regularization | 0.25 (StarAlignment) | 0.0 (exact interpolation) |
| Point limit (direct) | ~2000-3000 | No explicit limit |
| Large N handling | RecursivePointSurfaceSpline (quadtree), SurfaceSimplifier (PCA) | Not implemented |
| Point simplification | PCA-based, ~86% reduction (e.g. 6986 -> 974) | Not implemented |
| Outlier rejection | Via weights? | Not implemented (SIP has it, TPS doesn't) |
| Per-point weights | Yes, modulate interpolation strength | Not implemented |

### Kernel Convention

Some implementations use `r^2 ln(r^2) = 2*r^2 ln(r)`. This differs only by a
constant factor of 2 absorbed into weights -- interpolation results are identical.
Our `r^2 ln(r)` (without the factor of 2) matches the dominant convention used by
Bookstein (1989), PixInsight, and ALGLIB. The `1/(8pi)` normalizing constant from
the exact Green's function is universally dropped in practical implementations.

### Solver Choice: LU vs Bunch-Kaufman

The TPS system is symmetric but **indefinite** (saddle-point structure due to zero
block). Cholesky cannot be used. LU with partial pivoting is correct and stable but
doesn't exploit symmetry (~2x unnecessary work). Bunch-Kaufman (LDLT) diagonal
pivoting is the optimal choice for symmetric indefinite systems -- PixInsight uses
it explicitly. For N < 500, the difference is negligible (sub-millisecond solves).

The `Vec<Vec<f64>>` allocation pattern creates N+4 separate heap allocations vs a
single flat allocation. One benefit: row swapping during pivoting is O(1) (pointer
swap) vs O(N) (memcpy) with flat arrays. Practical impact for N < 200: negligible.

### Normalization: Bounding-Box vs Mean-Centered

PixInsight normalizes using the **mean** of coordinates as center, scaled by
inverse of the largest containing circle radius. Our bounding-box midpoint is
nearly identical for uniform distributions but can underperform for heavily
clustered data (e.g., two groups of stars far apart) where the mean sits closer
to the data density while the bounding-box center sits equidistant regardless.
Low priority improvement.

### Regularization Default

Our default `lambda = 0.0` (exact interpolation) is reasonable for clean
matched-star data from RANSAC. PixInsight defaults to 0.25 for StarAlignment,
prioritizing smoothness over exactness. Matched star positions typically have
sub-pixel noise (0.1-0.5 pixel), so a small regularization (0.05-0.25) can
absorb measurement errors. Consider setting a non-zero default when integrating
TPS into the pipeline.

### Computational Complexity for Typical N

| Operation | N=100 | N=500 |
|-----------|-------|-------|
| System assembly O(N^2) | 10K ops | 250K ops |
| System solve O(N^3) | 1M ops | 125M ops |
| Per-point evaluation O(N) | 100 ops | 500 ops |
| Full 4K warp (direct) | ~830M ops | ~4.2B ops |

System solve is NOT a bottleneck for N < 500. Per-point evaluation IS the
bottleneck for image warping, which is why `DistortionMap` (grid + bilinear
O(1) per pixel) is essential. For N > 2000, point simplification
(a la PixInsight's SurfaceSimplifier) would be needed before solving.

---

## Detailed Code Review Findings

### SIP: What We Do Correctly

1. **Linear term exclusion**: `term_exponents()` only generates terms with
   `2 <= p+q <= order`, matching the SIP standard (Shupe 2005). Linear terms are
   absorbed by the homography/CD matrix. Some implementations (Astrometry.net,
   ASTAP/HNSky) include linear terms in the polynomial basis then extract them to
   the CD matrix post-fit. Our approach of excluding them entirely is cleaner and
   equivalent since the homography already captures the linear transform.

2. **Normalization**: `avg_distance()` normalization keeps polynomial basis values
   near O(1) in normalized coordinates. This is significantly better than
   Astrometry.net (which uses raw pixel offsets, leading to condition numbers in the
   millions for order 5). Similar in concept to LSST's `ScaledPolynomialTransform`.
   NumPy's `polyfit` documentation recommends centering (`x - x.mean()`) for the
   same reason.

3. **Dual solver with condition monitoring**: Cholesky for SPD normal equations with
   a condition estimate (`max(diag(L)) / min(diag(L)) < 1e5`), falling back to LU
   with partial pivoting if ill-conditioned. This catches the case where normalization
   is insufficient (narrow strips, collinear-ish points). The 1e5 threshold on L
   diagonal ratio corresponds to cond(A^T*A) ~1e10 which means ~10 digits of
   precision lost -- sensible for f64 with ~15 digits.

4. **MAD-based sigma clipping**: Using median absolute deviation rather than standard
   deviation for the clipping threshold is robust to the outliers being detected.
   `MAD_TO_SIGMA = 1.4826022` is the correct conversion factor for Gaussian
   distributions. The one-sided clip (`> median + threshold`) is correct for
   non-negative residual magnitudes.

5. **Minimum point requirement**: `3 * terms.len()` is more conservative than
   Astrometry.net (1x) and prevents overfitting. For order 3 (7 terms), this
   requires at least 21 stars, which is reasonable. For order 5 (18 terms),
   54 stars are needed, matching the recommendation for HST-level data.

6. **Shared A^T*A matrix for both axes**: The design matrix depends only on
   positions, not on residuals, so the u and v corrections share the same
   `A^T*A`. Only `A^T*b` differs between axes. This is correct and efficient.

### SIP: Potential Issues (Minor)

1. **Residual computed in output space** (design choice, not bug): The targets
   `(target - transform(ref)) / norm_scale` live in the transform's output space.
   The correction polynomial is applied in input space. For transforms near identity,
   the Jacobian of the transform ~= I, so this is fine. For large rotations or
   scales significantly different from 1.0, there is a small systematic bias.
   Industry standard (LSST): fits the reverse polynomial first, then inverts via
   grid-sampling, which avoids this issue entirely. Our approximation is acceptable
   for registration (transforms are near-identity by definition of stacking).

2. **No automatic order selection**: The user must choose the SIP order. Industry
   practice (Astrometry.net tweak2.c): fit multiple orders, compare scatter, pick
   the best. Siril defaults to cubic (order 3) which is almost always appropriate.
   Our default of 3 is good. For a robust pipeline, fitting orders 2-4 and selecting
   by AIC/BIC or cross-validation would be ideal. Low priority.

3. **`monomial()` uses `powi()` for each term independently**: For high orders,
   computing `u^3` then `u^4` then `u^5` repeats work. Horner's scheme or
   precomputing powers `[u^0, u^1, ..., u^order]` would be faster. Given the tiny
   number of terms (max 18) and that this runs per-point (not per-pixel), the impact
   is negligible. Not worth optimizing.

### SIP: What We Don't Do But Should Consider

1. **Inverse polynomial (AP/BP)**: Needed for FITS WCS export. Standard approach
   (LSST): grid-sample `correction_at()` on 100x100 grid, fit the inverse polynomial
   at `AP_ORDER = A_ORDER + 1`. The inverse is not needed for our registration-only
   pipeline but blocks interoperability with DS9, SAOImage, astropy, and other FITS
   consumers. Medium priority if FITS export is planned.

2. **FITS header I/O**: Cannot read or write `A_i_j`, `B_i_j`, `AP_i_j`, `BP_i_j`
   keywords. This is separate from the fitting itself but needed for interop.

3. **Siril 1.4+ uses SIP undistortion during registration**: As of December 2025,
   Siril 1.4.0 applies WCS SIP undistortion during image registration, using
   Drizzle inherited from HST. This validates our approach of applying SIP
   corrections per-pixel during warping. Siril defaults to cubic (order 3) SIP,
   matching our default.

### TPS: What We Do Correctly

1. **Standard formulation**: The system matrix `[K+lambda*I, P; P^T, 0]` with
   kernel `U(r) = r^2 ln(r)` is the canonical TPS formulation from Bookstein (1989).

2. **Coordinate normalization**: Bounding-box normalization to [-1, 1] prevents
   the kernel `r^2 ln(r)` from amplifying scale differences (e.g., r=7200 gives
   ~4.6e8 while affine terms are O(6000)). This is critical for conditioning.

3. **Degenerate case handling**: Returns `None` for <3 points, collinear points
   (singular P matrix), and singular systems. The minimum 3 points is correct
   (matches the 3 affine DOF of the TPS basis).

4. **DistortionMap for O(1) evaluation**: Pre-computing distortion on a grid with
   bilinear interpolation is the standard approach for avoiding O(N) per-pixel
   cost during warping. This is how PixInsight handles large N cases too.

### TPS: Potential Issues

1. **No outlier rejection**: SIP has MAD-based sigma clipping but TPS does not.
   When integrating TPS, add iterative rejection. With regularized TPS
   (lambda > 0), residuals at control points are non-zero and can be used for
   outlier detection.

2. **No point limit or simplification**: For N > ~500, the O(N^3) solver becomes
   noticeable, and for N > 2000, it dominates. PixInsight uses SurfaceSimplifier
   (PCA-based, ~86% point reduction) and RecursivePointSurfaceSpline (quadtree)
   for large N. Our code has no explicit limit; integrating without a safeguard
   could cause unexpected slowness with many stars.

3. **Default regularization = 0**: Exact interpolation amplifies measurement noise.
   Matched star centroids have 0.1-0.5 pixel uncertainty. PixInsight defaults to
   lambda=0.25 for a reason -- it smooths out noise. When integrating TPS, default
   to a small positive regularization (0.05-0.25).

---

## Cross-Cutting Issues

### Issue 1: Stale README.md files (documentation)

**`distortion/README.md`** references methods that do not exist in the code:
`compute_inverse`, `inverse_correct`, `inverse_correction_at`,
`inverse_correct_points`, `has_inverse`. Claims wrong test counts.

**`sip/README.md`** states "Outlier rejection: None" but sigma-clipping is now
implemented. References `fit_residuals` method that does not exist. Lists
"Suggested improvements" that are already done (sigma-clipping, fit diagnostics).

**Fix**: Update both README.md files or delete them (NOTES-AI.md covers content).

### Issue 2: No inverse polynomial (AP/BP) for SIP

Not needed for registration-only pipeline but blocks future FITS export.
Standard approach: grid-sample `correction_at()` on 100x100 grid, fit inverse
polynomial at `AP_ORDER = A_ORDER + 1`. Low priority.

### Issue 3: TPS not integrated into pipeline

Fully implemented and tested (23 tests) but has `#![allow(dead_code)]`. When
integrating, should add:
- Sigma-clipping / outlier rejection (SIP has it, TPS doesn't)
- Per-point weights from star quality
- Non-zero regularization default (0.1-0.25)
- Point simplification for N > 1000

### Issue 4: Inconsistent solver allocations

SIP uses flat `[f64; MAX_ATA]` stack arrays (efficient, fixed-size).
TPS uses `Vec<Vec<f64>>` heap allocation (N+4 separate allocations).
Could unify to flat `Vec<f64>` with row-major indexing for TPS. Low priority.

### Issue 5: TPS lacks outlier rejection

SIP has MAD-based sigma-clipping but TPS does not. When integrating TPS, should
add iterative rejection. With regularized TPS (lambda > 0), residuals at control
points are non-zero and can be used for outlier detection.

---

## What We Do That's Not Needed (Unnecessary Complexity)

None identified. The code is lean:
- Both SIP and TPS implementations are straightforward with no over-engineering.
- The dual solver (Cholesky + LU fallback) might seem complex but the LU fallback
  is essential for ill-conditioned cases (narrow point distributions, near-collinear).
- `MAX_TERMS = 18` / `MAX_ATA = 324` fixed arrays avoid heap allocation for SIP,
  which is appropriate given the small fixed maximum (order 5).
- `DistortionMap` bilinear interpolation is the simplest adequate approach.

---

## What We Do Incorrectly (Bugs / Wrong Algorithms)

No bugs found. The implementations are algorithmically correct:
- SIP polynomial formula matches the Shupe 2005 standard.
- TPS formulation matches Bookstein 1989 / Eberly / Wikipedia.
- Cholesky decomposition is textbook correct with proper condition monitoring.
- LU with partial pivoting is textbook correct.
- MAD-to-sigma conversion factor 1.4826 is the standard value.
- Sigma clipping is one-sided (correct for non-negative residuals).
- Normal equations exploit symmetry (upper triangle + mirror).

The residual-space approximation (output space instead of input space) is a
design choice documented above, not a bug. It's standard for near-identity
transforms and matches how SIP is typically used in registration pipelines.

---

## Prioritized Improvements

### Priority 1 (documentation bugs)

1. Fix stale `distortion/README.md` -- remove references to nonexistent methods.
2. Fix stale `sip/README.md` -- update comparison table, remove `fit_residuals`,
   mark sigma-clipping and fit diagnostics as implemented.

### Priority 2 (TPS pipeline integration)

3. Add sigma-clipping to TPS (consistency with SIP).
4. Set non-zero regularization default (0.1) for pipeline use.
5. Add per-point weights for star quality modulation.
6. Remove `#![allow(dead_code)]` and wire into registration pipeline.
7. Add point count limit / warning for N > 1000.

### Priority 3 (nice-to-have)

8. Implement inverse polynomial (AP/BP) for future FITS export.
9. Automatic order selection for SIP (fit 2-4, pick by AIC/BIC).
10. Mean-centered normalization for TPS (match PixInsight).
11. Flatten TPS solver allocation (`Vec<Vec<f64>>` -> `Vec<f64>`).

### Not needed

- **Chebyshev/Legendre basis**: SIP standard requires power-basis polynomials.
- **Tikhonov regularization for SIP**: Overfitting addressed by 3x requirement + sigma-clipping.
- **QR decomposition for SIP**: Normal equations + condition monitoring sufficient for n <= 18.
- **Bunch-Kaufman for TPS**: LU is correct, difference negligible for N < 500.
- **Brown-Conrady model**: SIP order 3 subsumes radial+tangential distortion. Brown-Conrady
  uses `r' = r(1 + k1*r^2 + k2*r^4 + ...)` plus tangential terms `p1, p2`, all of which
  map to degree-3 and degree-5 SIP polynomials. No separate implementation needed.
- **TPV convention**: Sky-space corrections, different use case from pixel-space registration.
- **Zernike polynomials**: Describe wavefront errors, not geometric pixel distortion.
- **Alternative RBF kernels**: TPS is the parameter-free default recommended by ALGLIB and PixInsight.
- **Fast multipole / Nystrom**: Only needed for N > 5000; our DistortionMap handles evaluation.
- **GCV for lambda selection**: Fixed small lambda sufficient for star registration.

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
| Outlier rejection | MAD sigma-clipping | Not implemented yet |
| Industry use | Astrometry.net, Siril, HST, LSST | PixInsight, medical imaging |

**Default**: SIP order 3 (handles barrel/pincushion + mustache distortion).
Use TPS for wide-field mosaics with complex distortion, or when SIP residuals
remain high. Astrometry.net defaults to SIP order 2; PixInsight defaults to TPS
with lambda=0.25; Siril defaults to SIP order 3 (cubic).

---

## File Structure

```
distortion/
  mod.rs           Re-exports: SipConfig, SipFitResult, SipPolynomial,
                   TpsConfig, ThinPlateSpline, DistortionMap, tps_kernel (pub(crate))
  NOTES-AI.md      This file
  README.md        Human-readable overview (STALE -- references nonexistent methods)
  sip/
    mod.rs         SipPolynomial, SipFitResult, SipConfig, solvers, helpers (~580 lines)
    README.md      SIP standard overview (STALE -- claims no sigma-clipping)
    tests.rs       ~40 tests: barrel, pincushion, orders, edge cases, CRPIX,
                   sigma-clipping, fit diagnostics, solver tests
  tps/
    mod.rs         ThinPlateSpline, TpsConfig, DistortionMap, tps_kernel,
                   solve_linear_system (~445 lines)
    tests.rs       23 tests: exact interpolation, transforms (translation,
                   scaling, rotation, identity), barrel distortion,
                   regularization, edge cases, DistortionMap
```

## References

### SIP Standard
- [SIP Convention v1.0 (Shupe et al. 2005)](https://irsa.ipac.caltech.edu/data/SPITZER/docs/files/spitzer/shupeADASS.pdf)
- [SIP FITS Registry](https://fits.gsfc.nasa.gov/registry/sip.html)
- [SIP Convention (STWCS docs)](https://stwcs.readthedocs.io/en/latest/fits_convention_tsr/source/sip.html)
- [Astropy SIP Note](https://docs.astropy.org/en/stable/wcs/note_sip.html)
- [Using SIP Coefficients (HNSky)](https://www.hnsky.org/sip.htm)

### Industry Implementations
- [Astrometry.net fit-wcs.c](https://github.com/dstndstn/astrometry.net/blob/main/util/fit-wcs.c) -- QR solver, no normalization
- [Astrometry.net sip-utils.c](https://github.com/dstndstn/astrometry.net/blob/main/util/sip-utils.c) -- inverse polynomial computation
- [Astrometry.net Order Selection](https://groups.google.com/g/astrometry/c/3y2HoRTNXN8)
- [LSST FitSipDistortionTask](https://pipelines.lsst.io/modules/lsst.meas.astrom/tasks/lsst.meas.astrom.FitSipDistortion.html)
- [Siril Platesolving (SIP order 1-5)](https://siril.readthedocs.io/en/stable/astrometry/platesolving.html)
- [Siril Registration](https://siril.readthedocs.io/en/stable/preprocessing/registration.html)
- [PixInsight SurfaceSpline API](https://pixinsight.com/developer/pcl/doc/html/classpcl_1_1SurfaceSpline.html)
- [PixInsight StarAlignment Distortion](https://www.pixinsight.com/tutorials/sa-distortion/index.html)
- [PixInsight Solver Distortion](https://pixinsight.com/tutorials/solver-distortion/)

### Related Standards
- [TPV WCS Convention](https://fits.gsfc.nasa.gov/registry/tpvwcs/tpv.html)
- [SIP to PV Conversion (Shupe et al. 2012)](https://web.ipac.caltech.edu/staff/shupe/reprints/SIP_to_PV_SPIE2012.pdf)
- [Brown-Conrady Distortion Model](https://www.tangramvision.com/blog/the-innovative-brown-conrady-model)

### Numerical Methods
- [Normal Equations Stability (GSL)](https://www.gnu.org/software/gsl/doc/html/lls.html)
- [Normal Equations (Driscoll)](https://tobydriscoll.net/fnc-julia/leastsq/normaleqns.html)
- [Polynomial Fitting Stability (Driscoll)](https://tobydriscoll.net/fnc-julia/globalapprox/stability.html)
- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
- [GNU Astro sigma clipping](https://www.gnu.org/software/gnuastro/manual/html_node/Sigma-clipping.html)

### Thin Plate Splines
- [Thin Plate Spline (Wikipedia)](https://en.wikipedia.org/wiki/Thin_plate_spline)
- [TPS (Wolfram MathWorld)](https://mathworld.wolfram.com/ThinPlateSpline.html)
- [TPS Geometric Tools (Eberly)](https://www.geometrictools.com/Documentation/ThinPlateSplines.pdf)
- [Bookstein 1989: Principal Warps](https://user.engineering.uiowa.edu/~aip/papers/bookstein-89.pdf)
- [ALGLIB TPS](https://www.alglib.net/thin-plate-spline-interpolation-and-fitting/)
- [Bunch-Kaufman for Saddle-Point Systems (SIAM)](https://epubs.siam.org/doi/10.1137/S0895479897321088)
