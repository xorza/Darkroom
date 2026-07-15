# SIP Distortion Correction

SIP models nonlinear pixel-coordinate distortion with regular power-basis polynomials around a
reference point. Lumos fits the forward correction needed by registration and warping; it does not
read or write FITS SIP header keywords or fit an inverse AP/BP polynomial.

## Configuration

`SipConfig` owns:

- `order`: polynomial order from 2 through 5;
- `reference_point`: explicit polynomial origin, or the matched-point centroid;
- `clip_sigma`: MAD-scaled residual clipping threshold;
- `clip_iterations`: maximum robust refit passes.

An explicit image CRPIX makes the coefficient origin compatible with the FITS convention. The
default centroid is appropriate for internal correction and improves conditioning when no WCS
origin is available.

## Fit

`SipPolynomial::fit_from_transform` returns `Result<Option<SipFitResult>, RegistrationError>`:

- `Err` means invalid configuration or a fit failure;
- `Ok(None)` means too few usable points for the configured polynomial;
- `Ok(Some(...))` contains the fitted polynomial and diagnostics.

Coordinates and residuals are normalized around the selected reference point. The small normal
systems use Cholesky decomposition with LU fallback. Iterative MAD clipping removes marginal linear
transform inliers that do not support the polynomial fit.

`SipFitResult` reports RMS and maximum residual, accepted/rejected point counts, and maximum sampled
correction. `SipPolynomial::correct` applies the forward correction before the linear transform;
`WarpTransform` uses that composition for inverse-mapped resampling.

## Polynomial terms

Only terms with total degree at least two are included because translation and the linear terms are
already represented by `Transform`. Order 2 captures quadratic residuals. Radial barrel or
pincushion displacement contains cubic terms (`u * r²`, `v * r²`) and needs order 3 or higher.

## Layout

- `mod.rs`: configuration, fit, polynomial evaluation, residual and correction diagnostics.
- `tests.rs`: exact synthetic polynomial recovery, clipping, invalid configuration, and boundary
  cases.
