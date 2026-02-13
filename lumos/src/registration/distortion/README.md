# Distortion Module - AI Implementation Notes

## Overview

SIP polynomial distortion correction (FITS WCS standard) and non-parametric thin-plate spline (TPS) interpolation for correcting optical distortions in astronomical images.

## Distortion Types

### SIP Polynomial (`sip/`)
**SIP (Simple Imaging Polynomial)** convention — the industry standard for FITS WCS distortion:
```
u' = u + Σ A_pq × u^p × v^q    (for 2 ≤ p+q ≤ order)
v' = v + Σ B_pq × u^p × v^q    (for 2 ≤ p+q ≤ order)
```

- Used by Spitzer, HST, Astrometry.net, Siril, ASTAP
- No linear terms (those are in the CD matrix / homography)
- Order 2 (3 terms): Handles purely quadratic distortion
- Order 3 (7 terms): Handles barrel/pincushion (the standard case)
- Order 5 (18 terms): Handles mustache and higher-order distortion
- Coordinate normalization for numerical stability (average distance -> 1.0)
- Cholesky decomposition for normal equations, LU fallback
- Iterative sigma-clipping outlier rejection (MAD-based, 3-sigma)

**Important**: Barrel distortion residuals `dx = k*u*r^2` are **cubic** (order 3), not quadratic. Order 2 SIP cannot capture barrel/pincushion distortion -- use order 3 or higher.

**Pipeline integration**: When `RegistrationConfig.sip.enabled = true`, a SIP polynomial is fit to RANSAC inlier residuals after homography estimation. The correction is stored in `RegistrationResult.sip_correction` and applied when computing residuals.

### Thin-Plate Spline (`tps/`)
**Non-parametric** smooth interpolation:
```
f(x,y) = a0 + a1*x + a2*y + Sum_i w_i U(||(x,y) - (x_i,y_i)||)
```

- U(r) = r^2 log(r) radial basis function
- Minimizes bending energy for smooth interpolation
- LU decomposition for system solution
- Best for complex, non-radial distortions

## File Structure

```
distortion/
  mod.rs          Module declarations and re-exports only
  sip/
    mod.rs        SipPolynomial, SipConfig
    tests.rs      SIP tests (15 tests)
  tps/
    mod.rs        ThinPlateSpline, TpsConfig, DistortionMap, tps_kernel
    tests.rs      TPS tests (23 tests)
  NOTES-AI.md     This file
```

## Test Coverage

- sip/tests.rs: 15 tests (barrel, pincushion, quadratic, mustache, numerical edge cases)
- tps/tests.rs: 23 tests (TPS integration tests)

## References

- [SIP Convention for FITS Distortion](https://fits.gsfc.nasa.gov/registry/sip/SIP_distortion_v1_0.pdf) -- Shupe et al. 2005
