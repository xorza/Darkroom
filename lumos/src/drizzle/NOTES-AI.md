# drizzle Module - Implementation Notes

## Overview
Implements Variable-Pixel Linear Reconstruction (Drizzle algorithm, Fruchter & Hook 2002).
Takes dithered input images with geometric transforms, shrinks input pixels into "drops"
(controlled by pixfrac), maps them onto a higher-resolution output grid (controlled by scale),
and accumulates weighted contributions. Supports four kernels: Square, Point, Gaussian, Lanczos.

## Architecture
- `DrizzleAccumulator`: Accumulate contributions from multiple frames, then finalize
- Axis-aligned rectangle overlap computation for weighted accumulation
- Homography support via full projective division (f64 intermediates)
- Per-channel weight tracking
- Builder pattern for configuration with validation
- `drizzle_stack()`: High-level API that loads images, applies transforms, and finalizes

## Reference Standard (STScI cdrizzlebox.c)

The STScI reference implementation (spacetelescope/drizzle) defines:
- `pixfrac`: ratio of drop linear size to input pixel (0.0-1.0), defined in input pixel units
- `scale` (STScI): ratio of output pixel size to input pixel size (< 1 means higher resolution)
- Drop corners at `center +/- 0.5 * pixfrac` in input coordinates, then all 4 corners transformed
- `boxer()` + `sgarea()` for polygon-pixel overlap (not axis-aligned bounding box)
- Jacobian computed per pixel: `jaco = 0.5 * ((x1-x3)*(y0-y2) - (x0-x2)*(y1-y3))`
- Weight scaled by `weight_scale / jaco` to conserve flux

Weight accumulation formula (from Starlink/IRAF docs):
```
W'_xy = a_xy * w_xy + W_xy
I'_xy = (a_xy * i_xy * w_xy + I_xy * W_xy) / W'_xy
```
Where a_xy = fractional overlap area, w_xy = input weight, I/W = running output values.

STScI outputs three products: `outsci` (science), `outwht` (weight map), `outcon` (context bitmask).

## Known Issues

### Critical: Drop Size Formula is Inverted
**Location:** `mod.rs:196`

```rust
let drop_size = pixfrac / scale;  // WRONG
```

**Analysis:** In this codebase, `scale` is a resolution multiplier (2.0 = 2x resolution),
which is the *inverse* of STScI's convention (where scale = output_pixel_size / input_pixel_size).

- `pixfrac` = drop size in input pixel units (same as STScI)
- Each output pixel = `1/scale` input pixels wide
- Drop in output pixels = `pixfrac / (1/scale)` = `pixfrac * scale`

**Should be:** `let drop_size = pixfrac * scale;`

With defaults (pixfrac=0.8, scale=2.0):
- Current: 0.4 output pixels (smaller than one output pixel)
- Correct: 1.6 output pixels (covers ~1.6 output pixels in each direction)

The inverted formula makes drops cover less than 1 output pixel, degenerating toward a point
kernel. Self-consistent normalization hides the error in uniform regions, but spatial flux
distribution is wrong. This defeats drizzle's primary benefit: resolution recovery through
proper sub-pixel flux distribution.

### Critical: Square Kernel is Actually Turbo Kernel
**Location:** `mod.rs:252-307`

Current implementation:
1. Transforms only the CENTER point of each input pixel
2. Creates axis-aligned bounding box of size `drop_size` in output space
3. Computes rectangle-rectangle overlap (axis-aligned)

This matches STScI's "Turbo" kernel definition: "similar to square but the box is always the
same shape and size on the output grid, and is always aligned with the X and Y axes" (DrizzlePac docs).

True Square kernel (per STScI cdrizzlebox.c):
1. Define 4 corners at `center +/- 0.5 * pixfrac` in input coordinates
2. Transform ALL 4 corners through the geometric mapping
3. Compute polygon-polygon overlap using `boxer()`/`sgarea()` (Sutherland-Hodgman style clipping)
4. Compute per-pixel Jacobian for flux conservation

The axis-aligned approximation is adequate when rotation between frames is small, but has
systematic flux distribution errors when field rotation is present.

**Fix:** Rename current `Square` to `Turbo`. Implement proper `Square` with 4-corner
transformation and polygon clipping. STScI supports 5 kernels: square, turbo, point,
gaussian, lanczos3. Siril also implements all 5.

### Major: Gaussian Kernel FWHM is Wrong
**Location:** `mod.rs:359`

```rust
let sigma = drop_size / 2.355; // FWHM to sigma
```

Per STScI DrizzlePac documentation: "Gaussian kernel with FWHM equal to the value of pixfrac,
measured in input pixels." The FWHM should be `pixfrac` in input pixel units, converted to
output pixel units as `pixfrac * scale`.

Current formula uses `drop_size` which is `pixfrac / scale` (already inverted), compounding
the error.

**Should be:** `let sigma = (pixfrac * scale) / 2.355;`

### Major: Lanczos Used Without Required Constraints
**Location:** `mod.rs:441-525`, config presets

Per STScI DrizzlePac docs: "This option should never be used for pixfrac!=1.0, and is not
recommended for scale!=1.0." Siril docs confirm: "Lanczos kernels should only be used with
scale == pixfrac == 1.0."

Current issues:
- No validation or warning when Lanczos is used with pixfrac != 1.0 or scale != 1.0
- `x3()` preset sets pixfrac=0.7 -- if combined with Lanczos, this violates the constraint
- Lanczos is a resampling kernel, not a flux-distributing kernel; it makes physical sense
  only at scale=pixfrac=1.0 where it performs optimal bandlimited interpolation

### Major: No Output Clamping
**Location:** `mod.rs:528-576` (finalize)

The Lanczos kernel can produce negative weights (lobes extend to negative values). After
normalization, output pixel values can go negative or exceed the input range. Per Siril docs,
clamping is the default for Lanczos to "avoid artefacts." STScI notes the interpolated signal
"can be negative even if all samples are positive" due to negative kernel lobes.

Should clamp output to [0.0, max_input_value] or at minimum to [0.0, +inf) after finalization.

### Moderate: No Context/Contribution Image
STScI DrizzlePac produces three outputs:
- `outsci`: science data (weighted mean)
- `outwht`: weight map (accumulated weights)
- `outcon`: bitmask of contributing frames per output pixel

Current implementation only produces data + coverage map (normalized weights).

Context image is important for:
- Identifying artifacts (pixels with only 1-2 contributing frames)
- Error/variance estimation
- Debugging alignment issues
- Rejection of outlier frames at specific pixels

### Moderate: No Per-Pixel Input Weights / Bad Pixel Masks
**Location:** `mod.rs:183` (`add_image` signature)

Current: Only scalar weight per frame. STScI accepts per-pixel weight maps (inverse variance).

Cannot mask:
- Bad/hot/dead pixels
- Cosmic ray hits
- Satellite trails
- Detector artifacts (column defects, etc.)

Per STScI: "The weight image contains information about bad pixels in the image (in that bad
pixels result in lower weight values)." This is critical for real-world astronomical data.

### Moderate: No Jacobian / Geometric Distortion Correction
**Location:** `mod.rs:196`

Current: Constant `drop_size` for all pixels across the image.

STScI computes local Jacobian determinant per pixel from the 4 transformed corners:
```c
jaco = 0.5 * ((xout[1]-xout[3])*(yout[0]-yout[2]) - (xout[0]-xout[2])*(yout[1]-yout[3]));
w = weight_scale / jaco;
```

For homographies with significant spatial variation in magnification (e.g., wide-field with
optical distortion), constant drop size will not conserve flux across the field. Effect is
proportional to the magnification variation across the image.

### Moderate: min_coverage Compared Against Raw Accumulated Weight
**Location:** `mod.rs:553`

```rust
if weight >= min_coverage {
```

`min_coverage` is documented and validated as 0.0-1.0, but compared against raw accumulated
weight which depends on: number of frames, overlap areas, and frame weights. With 10 frames
of weight 1.0 and full overlap, raw weight per pixel is ~10.0, making min_coverage=0.1
trivially satisfied.

**Fix:** Compare against `weight / max_weight` (normalized) or redefine the parameter as a
minimum absolute weight threshold and update documentation.

### Moderate: Weight Formula Differs from STScI Standard
**Location:** `mod.rs:299-302`

Current implementation uses a two-pass approach (accumulate then divide):
```rust
self.data[idx] += flux * pixel_weight;
self.weights[idx] += pixel_weight;
// ... later in finalize: output = data / weight
```

This is algebraically equivalent to the STScI incremental formula for equal-weight frames,
but differs in behavior: the STScI formula `I' = (a*i*w + I*W) / W'` naturally handles
variable input weights and is numerically stable for streaming accumulation. The current
approach is acceptable but should be verified for numerical stability with many frames
(thousands of f32 additions can accumulate rounding error).

### Minor: No Parallelism
Per-pixel loop is single-threaded. STScI DrizzlePac and Siril both process rows in parallel.
For a 4000x4000 image at scale=2, the output is 8000x8000 = 64M pixels per frame. With
multiple kernels evaluating per input pixel, this is a significant bottleneck.

Row-parallel processing with rayon would be straightforward for all kernel types since each
input pixel writes to a bounded region of the output (no read-modify-write conflicts between
distant rows, though adjacent rows need atomic adds or per-thread accumulators).

### Minor: into_interleaved_pixels Allocation on Every Frame
**Location:** `mod.rs:198`

Allocates a new interleaved pixel buffer for each frame. Could work with planar channel data
directly (AstroImage already stores data in planar format) to avoid the conversion overhead.

### Minor: Coverage Map Averages Across Channels
**Location:** `mod.rs:541-546`

Coverage is computed as average weight across channels. Standard practice: coverage is a
per-pixel spatial property independent of channel. All channels of the same pixel should have
identical coverage (same geometric overlap). If channels differ, it indicates a logic issue.

## Correct Implementations
- **Point kernel:** Correct -- single-pixel contribution at transformed center
- **Lanczos kernel function:** Correct sinc-windowed formula with singularity handling at x=0
- **Rectangle overlap:** Correct axis-aligned AABB intersection
- **Projective transform:** Correct with f64 precision and perspective division
- **Weighted normalization:** Correct weighted-mean formula (data/weight)

## Summary of Required Fixes (Priority Order)
1. Drop size: `pixfrac * scale` (not `pixfrac / scale`)
2. Rename Square to Turbo; implement true Square with 4-corner transform + polygon clipping
3. Fix Gaussian FWHM: `sigma = (pixfrac * scale) / 2.355`
4. Add Lanczos parameter validation (require pixfrac=1.0)
5. Add output clamping (at minimum for Lanczos kernel)
6. Fix min_coverage to compare against normalized weight

## References
- Fruchter & Hook 2002, PASP 114:144 -- Original drizzle paper
- STScI DrizzlePac Handbook (2025) -- https://hst-docs.stsci.edu/drizzpac
- STScI drizzle C library -- https://github.com/spacetelescope/drizzle (cdrizzlebox.c)
- Siril drizzle docs -- https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html
- Starlink DRIZZLE docs -- https://starlink.eao.hawaii.edu/docs/sun139.htx/sun139ss14.html
- STScI DrizzlePac kernel docs -- https://drizzlepac.readthedocs.io/en/latest/drizzlepac_api/adrizzle.html
