# drizzle Module - Implementation Notes

## Overview
Implements Variable-Pixel Linear Reconstruction (Drizzle algorithm, Fruchter & Hook 2002,
PASP 114:144). Takes dithered input images with geometric transforms, shrinks input pixels
into "drops" (controlled by pixfrac), maps them onto a higher-resolution output grid
(controlled by scale), and accumulates weighted contributions. Supports four kernels:
Square, Point, Gaussian, Lanczos.

## Architecture
- `DrizzleAccumulator`: Accumulate contributions from multiple frames, then finalize
- Axis-aligned rectangle overlap computation for weighted accumulation
- Homography support via full projective division (f64 intermediates)
- Per-channel weight tracking
- Builder pattern for configuration with validation
- `drizzle_stack()`: High-level API that loads images, applies transforms, and finalizes

## Reference Algorithm: Fruchter & Hook 2002

### Parameter Definitions (from the paper)

- **s** (scale): Ratio of the linear size of an output pixel to an input pixel.
  s < 1 means output pixels are smaller than input (higher resolution).
  Example: s = 0.5 means output pixels are half the size of input pixels (2x resolution).

- **p** (pixfrac): Ratio of the linear size of the "drop" to the input pixel size (before
  geometric distortion adjustment). Range 0.0-1.0. p = 1.0 is shift-and-add; p -> 0
  is interlacing.

- **Drop size on output grid**: The drop has linear size `p` in input pixel units. Since
  one input pixel maps to `1/s` output pixels, the drop covers `p/s` output pixels in
  each dimension. Equivalently, the drop area in output pixels = `(p/s)^2`.

### Convention Mapping: Paper vs This Codebase

| Paper (STScI)        | This codebase    | Relationship            |
|----------------------|------------------|-------------------------|
| s (scale)            | 1/scale          | s_paper = 1/scale_code  |
| p (pixfrac)          | pixfrac          | Same                    |
| Drop in output px    | p/s = pixfrac * scale | Same formula      |
| pscale_ratio (code)  | 1/scale          | pscale_ratio = s_paper  |

In the STScI C implementation (`cdrizzlebox.c`), `pscale_ratio` is the ratio of output
pixel size to input pixel size (same as `s` in the paper). The turbo kernel computes:
```c
pfo = pixel_fraction / pscale_ratio / 2.0;  // half-width of drop in output pixels
```
This gives `pfo = p / s / 2` -- the half-width of the drop in output pixel coordinates.
Full drop width = `p / s` output pixels.

Since this codebase uses `scale = 1/s` (resolution multiplier), the drop size in output
pixels is `pixfrac / (1/scale) = pixfrac * scale`.

### Weight Accumulation (Fruchter & Hook Eqs. 3-5; Starlink/IRAF docs)

The STScI C implementation uses an incremental update formula:
```
W'_xy = a_xy * w_xy + W_xy                          (Eq. 4)
I'_xy = (a_xy * i_xy * w_xy + I_xy * W_xy) / W'_xy  (Eq. 5)
```
Where:
- `a_xy` = fractional overlap area between drop and output pixel
- `w_xy` = input pixel weight (from weight map, scaled by `weight_scale / jaco`)
- `i_xy` = input pixel value (surface brightness)
- `I_xy`, `W_xy` = running output image and weight values

The `iscale` factor (Eq. 3) = `s^2` accounts for the change in pixel solid angle when
mapping to a different pixel scale. This rescales input data to preserve surface brightness.

### Jacobian for Flux Conservation

STScI computes a local Jacobian per pixel from 4 transformed corners:
```c
jaco = 0.5 * ((xout[1]-xout[3])*(yout[0]-yout[2]) - (xout[0]-xout[2])*(yout[1]-yout[3]));
w = weight_scale / jaco;
```
The Jacobian is the determinant of the local coordinate transformation -- it measures how
much the transformation stretches/compresses area at each pixel. Dividing the weight by the
Jacobian ensures flux conservation when the geometric mapping has spatially varying
magnification (e.g., projective distortion across a wide field).

### STScI Output Products

STScI produces three arrays:
- `outsci`: science data (weighted mean of input contributions)
- `outwht`: weight map (accumulated weights per pixel)
- `outcon`: context bitmask (32-bit integer, each bit = one input frame contributed)

## Reference Standard: STScI Kernel Implementations (cdrizzlebox.c)

### Square Kernel (true drizzle)
The original, mathematically exact kernel:
1. Define 4 corners at `center +/- 0.5 * pixfrac` in **input** coordinates
2. Transform ALL 4 corners through the geometric mapping to output coordinates
3. Compute polygon-polygon overlap using `boxer()` + `sgarea()` (Sutherland-Hodgman style)
4. Compute per-pixel Jacobian from the 4 transformed corners
5. Weight = `weight_scale / jaco`

Flux-preserving by construction: each input pixel's flux is distributed to output pixels
in exact proportion to the overlap area between the transformed quadrilateral and the
output pixel grid.

### Turbo Kernel (axis-aligned approximation)
Simplified version of Square:
1. Transform only the **center** point of each input pixel
2. Create axis-aligned rectangle of size `pixfrac / pscale_ratio` in output coordinates
3. Compute rectangle-rectangle overlap using `over()` function (AABB intersection)
4. Same weight formula but without per-pixel Jacobian (uses constant scale)

STScI code:
```c
pfo = pixel_fraction / pscale_ratio / 2.0;   // half-width in output pixels
ac = 1.0 / (pixel_fraction * pixel_fraction); // inverse drop area in input pixel units
dover_scale = ac * pscale_ratio * pscale_ratio; // normalization factor
```

"Similar to kernel='square' but the box is always the same shape and size on the output
grid, and is always aligned with the X and Y axes" (DrizzlePac docs). The default for
intermediate products in the HST pipeline due to speed. Adequate when rotation is small.

### Point Kernel
Degenerate case: each input pixel contributes to exactly one output pixel (the nearest
to the transformed center). Equivalent to pixfrac -> 0 limit. Fastest kernel but
requires very well-dithered data to avoid gaps.

### Gaussian Kernel
STScI code:
```c
kscale2 = pscale_ratio * pscale_ratio;
pfo = nsig * pixel_fraction / 2.3548 / pscale_ratio;  // nsig = 2.5
ac = 1.0 / (pixel_fraction * pixel_fraction);
gaussian_efac = (2.3548 * 2.3548) * kscale2 * ac / 2.0;
gaussian_es = gaussian_efac / M_PI;
// For each output pixel at distance r from mapped center:
dover = gaussian_es * exp(-r2 * gaussian_efac);
```

Per DrizzlePac docs: "Gaussian kernel with FWHM equal to the value of pixfrac, measured
in input pixels." The FWHM in output pixels = `pixfrac / pscale_ratio` = `pixfrac * scale`
(in this codebase's convention). The `sigma` in output pixels = FWHM / 2.3548.

The normalization `gaussian_es = gaussian_efac / PI` ensures the Gaussian integrates to
unity over the output plane, preserving flux.

Note: DrizzlePac warns "flux conservation cannot be guaranteed" with Gaussian kernels.

### Lanczos Kernel
STScI uses a LUT-based implementation:
```c
nlut = ceil(kernel_order / lut_delta) + 1;
create_lanczos_lut(kernel_order, nlut, lut_delta, lut);
// For each output pixel:
sdp = pscale_ratio / lut_delta;
ix = (int)(fabs(dx) * sdp + 0.5);
iy = (int)(fabs(dy) * sdp + 0.5);
dover = lut[ix] * lut[iy];
```
Supports kernel_order 2 or 3 (Lanczos-2 or Lanczos-3).

DrizzlePac docs: "should never be used for pixfrac != 1.0, and is not recommended for
scale != 1.0." Siril confirms: "Lanczos kernels should only be used when scale == pixfrac
== 1.0." At those settings it acts as an optimal bandlimited resampling filter.

## PixInsight DrizzleIntegration

PixInsight supports kernels: Square, Circular, Gaussian, Variable-Shape.
- Uses `dropShrink` parameter (equivalent to pixfrac)
- Recommended: `dropShrink ~ 1/scale` (e.g., 0.5 for 2x drizzle)
- For CFA data: recommends scale=1.0, pixfrac=1.0
- Variable-shape kernel: custom parametric shape, requires many frames
- Square kernel with dropShrink=1 recommended for small image sets

## Known Issues

### Critical: Drop Size Formula is Inverted
**Location:** `mod.rs:196`

```rust
let drop_size = pixfrac / scale;  // WRONG
```

**Analysis:** In this codebase, `scale` is a resolution multiplier (2.0 = 2x resolution),
which is the **inverse** of STScI's convention (s = output_pixel_size / input_pixel_size).

- `pixfrac` = drop size in input pixel units (same as STScI)
- This codebase: `scale` = `1/s`, so one output pixel = `1/scale` input pixels
- Drop in output pixels = `pixfrac / s` = `pixfrac / (1/scale)` = `pixfrac * scale`

**STScI turbo kernel confirms:**
```c
pfo = pixel_fraction / pscale_ratio / 2.0;
// pscale_ratio = s = 1/scale_code
// pfo = pixfrac / (1/scale) / 2 = pixfrac * scale / 2
```
Full drop width = `2 * pfo` = `pixfrac * scale`.

**Should be:** `let drop_size = pixfrac * scale;`

With defaults (pixfrac=0.8, scale=2.0):
- Current: `0.8 / 2.0 = 0.4` output pixels (too small, degenerates toward point kernel)
- Correct: `0.8 * 2.0 = 1.6` output pixels (drop covers ~1.6 output pixels each direction)

The inverted formula makes drops smaller than one output pixel. Self-consistent normalization
hides the error in uniform regions (flat image drizzles to a flat output), but spatial flux
distribution is wrong. This defeats drizzle's primary benefit: resolution recovery through
proper sub-pixel flux distribution from overlapping drops.

### Critical: Square Kernel is Actually Turbo Kernel
**Location:** `mod.rs:252-307`

Current implementation:
1. Transforms only the **center** point of each input pixel
2. Creates axis-aligned bounding box of size `drop_size` in output space
3. Computes rectangle-rectangle overlap (axis-aligned via `compute_square_overlap`)

This matches STScI's "Turbo" kernel exactly -- not the true Square kernel.

True Square kernel (per STScI `cdrizzlebox.c`):
1. Define 4 corners at `center +/- 0.5 * pixfrac` in **input** coordinates
2. Transform ALL 4 corners through the geometric mapping
3. Compute polygon-polygon overlap using `boxer()`/`sgarea()` (Sutherland-Hodgman clipping)
4. Compute per-pixel Jacobian for flux conservation

The axis-aligned approximation is adequate when rotation between frames is small (typical
for alt-az tracking with field rotation < 1 degree). For significant rotation or projective
distortion, the turbo kernel has systematic flux distribution errors because the actual
transformed pixel shape is a general quadrilateral, not an axis-aligned rectangle.

**Fix:** Rename current `Square` to `Turbo`. Implement proper `Square` with 4-corner
transformation and polygon clipping. STScI supports 5 kernels: square, turbo, point,
gaussian, lanczos3. Siril also implements all 5.

### Major: Gaussian Kernel FWHM is Wrong
**Location:** `mod.rs:359`

```rust
let sigma = drop_size / 2.355; // FWHM to sigma
```

Per STScI DrizzlePac: "Gaussian kernel with FWHM equal to the value of pixfrac, measured
in input pixels." The FWHM should be `pixfrac` in input pixel units, converted to output
pixel coordinates as `pixfrac * scale` (this codebase's convention).

STScI C code computes:
```c
pfo = nsig * pixel_fraction / 2.3548 / pscale_ratio;
// = nsig * pixfrac * scale / 2.3548   (in our convention)
```
The Gaussian sigma in output pixels = `pixfrac * scale / 2.3548`.

Current formula uses `drop_size` which is `pixfrac / scale` (already inverted), giving
sigma = `pixfrac / scale / 2.355`. With defaults: sigma = 0.17 output pixels instead of
the correct 0.68 output pixels. This makes the Gaussian far too narrow.

**Should be:** `let sigma = (pixfrac * scale) / 2.3548;`

Additionally, the STScI Gaussian normalization factor is:
```c
gaussian_efac = (2.3548^2) * kscale2 * ac / 2.0;
gaussian_es = gaussian_efac / PI;
```
This ensures the Gaussian integrates to 1.0 over the 2D plane. The current implementation
uses a two-pass normalization (sum weights, then divide), which achieves the same effect
but at the cost of iterating the kernel support twice per input pixel.

### Major: Lanczos Used Without Required Constraints
**Location:** `mod.rs:441-525`, config presets

Per STScI DrizzlePac docs: "This option should never be used for pixfrac!=1.0, and is not
recommended for scale!=1.0." Siril confirms: "Lanczos kernels should only be used with
scale == pixfrac == 1.0."

Current issues:
- No validation or warning when Lanczos is used with pixfrac != 1.0 or scale != 1.0
- `x3()` preset sets pixfrac=0.7 -- if combined with Lanczos, this violates the constraint
- Lanczos is a resampling kernel, not a flux-distributing kernel; it makes physical sense
  only at scale=pixfrac=1.0 where it performs optimal bandlimited interpolation

Also: the current Lanczos support radius formula is:
```rust
let radius = (a * drop_size / scale).max(a).ceil() as isize;
```
This uses `drop_size / scale` which is `pixfrac / scale^2` -- doubly wrong (inverted drop
size divided by scale again). STScI uses `sdp = pscale_ratio / lut_delta` for distance
scaling, which at pixfrac=1, scale=1 gives a support radius of exactly `a` (the kernel
order). The radius should simply be `a.ceil() as isize` when pixfrac=scale=1.

### Major: No Output Clamping
**Location:** `mod.rs:528-576` (finalize)

The Lanczos kernel has negative lobes (sinc function crosses zero). After normalization,
output pixel values can go negative or exceed the input range. Per Siril docs, clamping
is the default for Lanczos to "avoid artefacts." STScI notes the interpolated signal
"can be negative even if all samples are positive" due to negative kernel lobes.

Should clamp output to `[0.0, max_input_value]` or at minimum to `[0.0, +inf)` after
finalization. Alternatively, only clamp when using Lanczos kernel.

### Moderate: No Context/Contribution Image
STScI DrizzlePac produces three outputs:
- `outsci`: science data (weighted mean)
- `outwht`: weight map (accumulated weights)
- `outcon`: 32-bit bitmask of contributing frames per output pixel

Current implementation only produces data + normalized coverage map.

Context image is important for:
- Identifying artifacts (pixels with only 1-2 contributing frames)
- Error/variance estimation (need to know contributing frame count)
- Debugging alignment issues
- Rejection of outlier frames at specific pixels

Implementation: add a `Vec<u32>` to `DrizzleAccumulator`, OR each output pixel with
`(1 << frame_index)` when frame contributes. Supports up to 32 frames per u32 word.

### Moderate: No Per-Pixel Input Weights / Bad Pixel Masks
**Location:** `mod.rs:183` (`add_image` signature)

Current: Only scalar weight per frame. STScI accepts per-pixel weight maps (inverse
variance maps).

Cannot mask:
- Bad/hot/dead pixels
- Cosmic ray hits
- Satellite trails
- Detector artifacts (column defects, etc.)

Per STScI: "The weight image contains information about bad pixels in the image (in that
bad pixels result in lower weight values)." JWST pipeline uses per-pixel inverse variance
weighting as standard.

### Moderate: No Jacobian / Geometric Distortion Correction
**Location:** `mod.rs:196`

Current: Constant `drop_size` for all pixels across the image.

STScI computes local Jacobian determinant per pixel from the 4 transformed corners (see
the Jacobian formula above). For homographies with significant spatial variation in
magnification (e.g., wide-field telescopes with optical distortion), constant drop size
does not conserve flux across the field.

For a pure translation/rotation/similarity transform (no perspective), the Jacobian is
constant across the image, so the current approach is correct. The error grows with the
degree of projective distortion in the transform.

### Moderate: No Variance/Error Output
**Location:** `DrizzleResult` struct

JWST pipeline propagates three variance components through drizzle:
- Read noise variance
- Poisson noise variance
- Flat-field variance

Each is resampled separately and combined: for each variance component, the weighted
variance sum is divided by the total weight squared. Final error = sqrt(sum of variance
components). This allows proper uncertainty estimation on the drizzled output.

Current implementation has no variance propagation at all.

### Moderate: min_coverage Compared Against Raw Accumulated Weight
**Location:** `mod.rs:553`

```rust
if weight >= min_coverage {
```

`min_coverage` is documented and validated as 0.0-1.0, but compared against raw
accumulated weight which depends on: number of frames, overlap areas, and frame weights.
With 10 frames of weight 1.0 and full overlap, raw weight per pixel is ~10.0, making
min_coverage=0.1 trivially satisfied for all pixels.

**Fix:** Compare against `weight / max_weight` (normalized) or redefine the parameter as
a minimum absolute weight threshold and update documentation. STScI uses absolute weight
thresholds but documents them as such.

### Moderate: Weight Formula -- Two-Pass vs Incremental
**Location:** `mod.rs:299-302`

Current implementation uses a two-pass approach (accumulate then divide):
```rust
self.data[idx] += flux * pixel_weight;
self.weights[idx] += pixel_weight;
// ... later in finalize: output = data / weight
```

STScI uses incremental update:
```c
vc_new = vc + dow;
value = (old_value * vc + dow * d) / vc_new;
```

The two-pass approach is algebraically equivalent for equal-weight frames: final result
is `sum(flux_i * w_i) / sum(w_i)` either way. For numerical stability: the incremental
formula avoids accumulating very large sums in the numerator (with thousands of frames,
f32 sum of weighted fluxes may lose precision). However, the two-pass approach is simpler
and allows parallelism (atomic adds to accumulators, single finalization pass).

For the expected frame counts in amateur astrophotography (tens to hundreds of frames),
f32 precision is adequate.

### Minor: No Parallelism
Per-pixel loop is single-threaded. STScI DrizzlePac and Siril both process rows in
parallel. For a 4000x4000 image at scale=2, the output is 8000x8000 = 64M pixels per
frame.

**Parallelization strategy:**
The current loop iterates over input pixels, with each input pixel writing to a small
neighborhood of output pixels. This creates write conflicts between input pixels that
map to overlapping output regions. Options:
1. **Per-thread accumulators** (rayon `par_iter` + reduce): each thread gets its own
   output buffer, merge at the end. Memory cost: N_threads * output_size.
2. **Atomic adds** (`AtomicU32` with `f32::to_bits`/`from_bits`): lock-free but slow for
   heavy contention on the same output pixels.
3. **Output-driven loop**: iterate output pixels, find contributing input pixels. Requires
   inverse transform. Avoids write conflicts entirely. STScI uses input-driven loop.
4. **Row chunking**: split input rows into chunks, process in parallel. Adjacent chunks may
   write to overlapping output rows -- use mutex or padding rows.

### Minor: into_interleaved_pixels Allocation on Every Frame
**Location:** `mod.rs:198`

Allocates a new interleaved pixel buffer for each frame via `image.into_interleaved_pixels()`.
Could work with planar channel data directly (AstroImage stores data in planar format) to
avoid the per-frame O(W*H*C) allocation and copy.

### Minor: Coverage Map Averages Across Channels
**Location:** `mod.rs:541-546`

Coverage is computed as average weight across channels. Standard practice: coverage is a
per-pixel spatial property independent of channel. All channels of the same pixel should
have identical coverage (same geometric overlap). If channels differ, it indicates either
per-channel weight variation or a logic issue. Should use a single weight per spatial pixel.

## Correct Implementations
- **Point kernel:** Correct -- single-pixel contribution at transformed center
- **Lanczos kernel function:** Correct sinc-windowed formula with singularity handling at x=0
- **Rectangle overlap (`compute_square_overlap`):** Correct axis-aligned AABB intersection
- **Projective transform (`transform_point`):** Correct with f64 precision and perspective division
- **Two-pass weighted normalization:** Correct weighted-mean formula (data/weight)
- **Builder pattern config:** Correct with pixfrac/min_coverage range validation
- **Output dimension calculation:** Correct: `ceil(input_dim * scale)`

## Summary of Required Fixes (Priority Order)

1. **Drop size formula** (Critical): `pixfrac * scale` not `pixfrac / scale`
2. **Rename Square to Turbo** (Critical): current implementation is axis-aligned (Turbo);
   implement true Square with 4-corner transform + polygon clipping
3. **Gaussian FWHM** (Major): `sigma = (pixfrac * scale) / 2.3548`
4. **Lanczos constraints** (Major): validate/warn when pixfrac != 1.0 or scale != 1.0;
   fix support radius formula
5. **Output clamping** (Major): clamp to [0, +inf) after finalization, at least for Lanczos
6. **min_coverage semantics** (Moderate): compare against normalized weight
7. **Context image** (Moderate): add per-pixel contributing-frame bitmask
8. **Per-pixel weight maps** (Moderate): accept `Option<&[f32]>` weight map per frame
9. **Jacobian correction** (Moderate): compute local Jacobian for non-affine transforms
10. **Variance output** (Moderate): propagate variance through drizzle
11. **Parallelism** (Minor): rayon row-parallel processing
12. **Avoid interleaved copy** (Minor): work with planar data directly

## References
- Fruchter & Hook 2002, PASP 114:144-152 -- Original drizzle paper (arXiv: astro-ph/9808087)
- STScI DrizzlePac Handbook -- https://hst-docs.stsci.edu/drizzpac
- STScI drizzle C library (cdrizzlebox.c) -- https://github.com/spacetelescope/drizzle
- STScI DrizzlePac kernel docs -- https://drizzlepac.readthedocs.io/en/latest/drizzlepac_api/adrizzle.html
- Siril drizzle docs -- https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html
- Starlink DRIZZLE docs -- https://starlink.eao.hawaii.edu/docs/sun139.htx/sun139ss14.html
- JWST Resample step -- https://jwst-pipeline.readthedocs.io/en/stable/jwst/resample/main.html
- PixInsight PCL DrizzleIntegration -- https://github.com/PixInsight/PCL
- WFC3 ISR 2015-04: Optimizing pixfrac -- https://www.stsci.edu/files/live/sites/www/files/home/hst/instrumentation/wfc3/documentation/instrument-science-reports-isrs/_documents/2015/WFC3-2015-04.pdf
