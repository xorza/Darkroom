# SIP (Simple Imaging Polynomial) Distortion Correction

## Standard

The [SIP convention](https://fits.gsfc.nasa.gov/registry/sip/SIP_distortion_v1_0.pdf) (Shupe et al. 2005) represents non-linear geometric distortion as polynomials in FITS header keywords. It is the de facto standard for astronomical imaging, used by Spitzer, HST, Astrometry.net, LSST, Siril, and ASTAP.

### Forward transform (pixel to corrected pixel)

```
u = pixel_x - CRPIX1
v = pixel_y - CRPIX2

u' = u + SUM(A_pq * u^p * v^q)   for 2 <= p+q <= A_ORDER
v' = v + SUM(B_pq * u^p * v^q)   for 2 <= p+q <= B_ORDER
```

The corrected coordinates (u', v') are then multiplied by the CD matrix to get intermediate world coordinates.

### Inverse transform (corrected pixel to pixel)

```
u = U + SUM(AP_pq * U^p * V^q)   for 2 <= p+q <= AP_ORDER
v = V + SUM(BP_pq * U^p * V^q)   for 2 <= p+q <= BP_ORDER
```

where U, V are the result of applying the inverse CD matrix to intermediate world coordinates.

### Key properties

- Reference point is always CRPIX (image center)
- Linear terms (p+q < 2) are excluded: already captured by the CD matrix
- Only regular polynomials (no Chebyshev/Legendre basis)
- Inverse polynomial is an approximation, not an exact inverse
- Double precision required (single precision strongly discouraged)

## Implementation comparison

### How reference implementations work

| Aspect | Astrometry.net | LSST | This implementation |
|---|---|---|---|
| **Solver** | QR decomposition | Scaled polynomial + affine | Cholesky, LU fallback |
| **Normalization** | None (raw pixel offsets) | Scaled polynomial intermediate repr. | avg-distance normalization |
| **Outlier rejection** | Per-star weights | Sigma-clipping (3 iterations) | None (relies on RANSAC upstream) |
| **Inverse coefficients** | Grid sampling + least-squares | Grid sampling (100x100) | Not implemented |
| **Reference point** | CRPIX (fixed) | Match centroid | Configurable (default: centroid) |
| **Residual direction** | `target - transform(ref)` | Similar | `transform(ref) - target` (negated in fit) |

### What this implementation does well

- **Coordinate normalization**: Normalizes u,v by average distance from reference point so polynomial basis values stay near O(1). This is better than astrometry.net's raw pixel offsets for numerical conditioning.
- **Dual solver strategy**: Cholesky for SPD normal equations with LU fallback when the matrix is not positive definite. Practical and robust.
- **Normal equations approach**: Efficient for the small systems involved (3-18 unknowns). Building A^T*A directly avoids storing the full m-by-n design matrix.
- **Clean API**: `fit_residuals` for post-RANSAC usage, `fit_from_transform` for convenience, `correct`/`correction_at` for evaluation.
- **Residual normalization**: Both coordinates and residuals are normalized by the same scale, keeping coefficients well-conditioned.

### Gaps relative to the standard

1. **No inverse polynomial (AP, BP)**: The standard defines inverse coefficients for sky-to-pixel transforms. This implementation only has the forward correction. Needed for: FITS header export, round-trip coordinate transforms, interoperability with other tools.

2. **Reference point is centroid, not CRPIX**: The SIP standard mandates CRPIX as the polynomial origin. Using the centroid of input points works for internal correction but produces coefficients incompatible with FITS headers. Any future FITS export would need to re-fit with CRPIX as origin.

3. **No FITS header I/O**: Cannot read A_pq/B_pq from FITS headers or write them. This limits interoperability with tools that consume SIP headers (DS9, SAOImage, astropy, etc.).

## Suggested improvements

### 1. Inverse polynomial computation (AP, BP)

Compute inverse coefficients by grid sampling, following astrometry.net's approach:

1. Lay down a grid across the image (e.g., 10*(order+1) per side)
2. Apply forward SIP to each grid point to get warped coordinates
3. Fit inverse polynomial from warped coordinates back to original positions
4. Validate round-trip error (forward then inverse) at random points

This enables FITS header export and sky-to-pixel coordinate transforms.

### 2. QR decomposition solver

The normal equations approach squares the condition number of the design matrix. For order 5 with points near image edges, this can matter. Astrometry.net uses QR decomposition on the full design matrix. Given the small system sizes (max 18 unknowns), QR would be equally fast and more numerically robust.

Alternatively, SVD would provide the same benefit and also expose the singular values, making it easy to detect rank deficiency.

### 3. Outlier-aware fitting

The current implementation assumes clean inlier sets (from RANSAC). Adding optional sigma-clipping (as LSST does with 3 iterations, configurable rejection threshold) would make the fit more robust to marginal inliers that RANSAC let through.

### 4. Fit quality diagnostics

Add a method to compute fit quality metrics after fitting:

- RMS and max residual at the input points
- RMS and max residual at a denser grid (interpolation quality)
- Condition number of the normal equations matrix

This helps users decide whether to increase/decrease order.

### Declined

- **Chebyshev/Legendre basis**: The SIP standard explicitly requires regular power-basis polynomials. Using orthogonal polynomials internally would improve conditioning but require conversion for FITS export, adding complexity for marginal gain given the low orders (2-5) and existing normalization.
- **Regularization (Tikhonov/ridge)**: For orders 2-5 with 80+ RANSAC inliers, overfitting is not a practical concern. The normalization already addresses conditioning. Adding a regularization parameter would complicate the API without clear benefit for this use case.

## References

- [SIP Convention v1.0](https://fits.gsfc.nasa.gov/registry/sip/SIP_distortion_v1_0.pdf) -- Shupe et al. 2005
- [Astrometry.net SIP implementation](https://github.com/dstndstn/astrometry.net/blob/main/util/sip-utils.c)
- [LSST FitSipDistortionTask](https://pipelines.lsst.io/modules/lsst.meas.astrom/tasks/lsst.meas.astrom.FitSipDistortion.html)
- [Astropy SIP note](https://docs.astropy.org/en/latest/wcs/note_sip.html)
- [HNSKY SIP usage](https://www.hnsky.org/sip.htm)
- [STWCS SIP convention](https://stwcs.readthedocs.io/en/latest/fits_convention_tsr/source/sip.html)
