# drizzle Module - Implementation Notes

## Overview
Implements Variable-Pixel Linear Reconstruction (Drizzle algorithm, Fruchter & Hook 2002,
PASP 114:144). Takes dithered input images with geometric transforms, shrinks input pixels
into "drops" (controlled by pixfrac), maps them onto a higher-resolution output grid
(controlled by scale), and accumulates weighted contributions. Supports four kernels:
Turbo, Point, Gaussian, Lanczos.

## Architecture
- `DrizzleAccumulator`: Accumulate contributions from multiple frames, then finalize
  - `data: ArrayVec<Buffer2<f32>, 3>` -- per-channel accumulated weighted flux
  - `weights: ArrayVec<Buffer2<f32>, 3>` -- per-channel accumulated weights
  - `new(input_dims: ImageDimensions, config)` / `dimensions() -> ImageDimensions`
- `DrizzleResult.coverage: Buffer2<f32>` -- normalized [0,1] coverage map
- `add_image()` consumes `AstroImage` (owned); kernel methods borrow `&AstroImage`
- No interleaved allocation -- kernel inner loops use `image.channel(c)[(ix, iy)]` directly
- Transform via `Transform::apply(DVec2) -> DVec2` (projective, f64 precision, cast to f32)
- Builder pattern for configuration with validation
- `drizzle_stack()`: High-level API that loads images, applies transforms, and finalizes
- Rayon-parallel finalization (row-parallel normalization and coverage computation)
- Output built via `AstroImage::from_planar_channels()` (no interleaving step)

## Kernels

### Turbo (default)
Axis-aligned rectangular drop centered on the transformed pixel center. Transforms only
the center point, creates axis-aligned bounding box of size `drop_size` in output space.
Fast and adequate when rotation between frames is small. Named "turbo" per STScI DrizzlePac
convention -- STScI's "square" kernel uses full polygon clipping (not implemented here).

### Point
Degenerate case: each input pixel contributes to exactly one output pixel (the nearest
to the transformed center). Fastest kernel, requires very well-dithered data.

### Gaussian
Gaussian droplet with FWHM equal to `pixfrac` in input pixels (= `pixfrac * scale` in
output pixels). sigma = `drop_size / 2.3548`. Uses two-pass normalization: first computes
total Gaussian weight across the kernel support, then distributes flux proportionally.
DrizzlePac warns "flux conservation cannot be guaranteed" with Gaussian kernels.

### Lanczos
Lanczos-3 kernel for high-quality bandlimited interpolation. Support radius = 3 pixels.
**Only valid at pixfrac=1.0, scale=1.0** -- a runtime warning is emitted if these
constraints are violated. Output is clamped to `[0, +inf)` in `finalize()` to suppress
negative ringing artifacts from the sinc lobes.

## Key Formulas

### Drop Size
```rust
let drop_size = pixfrac * scale;  // in output pixels
```
- `pixfrac` = fraction of input pixel size (0.0-1.0)
- `scale` = output resolution multiplier (e.g., 2.0 for 2x)
- Drop area in output pixels = `drop_size^2`
- STScI equivalent: `pfo = pixel_fraction / pscale_ratio / 2.0` (half-width)
  where `pscale_ratio = 1/scale`

### Convention Mapping: Paper vs This Codebase

| Paper (STScI)        | This codebase    | Relationship            |
|----------------------|------------------|-------------------------|
| s (scale)            | 1/scale          | s_paper = 1/scale_code  |
| p (pixfrac)          | pixfrac          | Same                    |
| Drop in output px    | p/s = pixfrac * scale | Same formula      |
| pscale_ratio (code)  | 1/scale          | pscale_ratio = s_paper  |

### Weight Accumulation
Two-pass approach: accumulate `data += flux * weight` and `weights += weight`, then in
finalize compute `output = data / weight`. Algebraically equivalent to STScI single-pass
incremental formula `I' = (I*W + i*a*w) / (W + a*w)` for the expected frame counts in
amateur astrophotography (tens to hundreds).

### min_coverage
Compared against normalized weight: `weight_threshold = min_coverage * max_weight`.
Coverage map uses channel 0 weight as representative (all channels have identical
geometric overlap). Normalized to [0, 1] in the output.

## STScI Reference Implementation (cdrizzlebox.c)

### Core Accumulation (update_data_var)
```c
// Flux: weighted incremental average
value = (output_data[ii,jj] * vc + dow * d) / (vc + dow);
// Variance: squared-weight propagation (3 components: read, Poisson, flat)
var_new = (old_var * vc^2 + dow^2 * new_var) / (vc + dow)^2;
// DQ: bitwise OR of contributing frames
output_dq |= input_dq;
```

### Turbo Kernel (matches our implementation)
```c
pfo = pixel_fraction / pscale_ratio / 2.0;   // half-width in output pixels
ac = 1.0 / (pixel_fraction * pixel_fraction); // inverse drop area in input pixel units
dover_scale = ac * pscale_ratio * pscale_ratio;
dow = over(ii, jj, xxi, xxa, yyi, yya) * dover_scale * w;
```

### Gaussian Kernel (differs from ours)
```c
// STScI: analytical normalization with pi-based constant
pfo = nsig * pixel_fraction / 2.3548 / pscale_ratio;  // nsig = 2.5 (wider support)
gaussian_efac = (2.3548^2) * kscale2 * ac / 2.0;
gaussian_es = gaussian_efac / M_PI;
dover = gaussian_es * exp(-r2 * gaussian_efac);  // single-pass, no normalization loop
// Ours: per-pixel normalization (two-pass, sum of Gaussian weights = 1.0)
```

### Square Kernel (NOT implemented)
```c
// Transform ALL 4 corners through geometric mapping
jaco = 0.5 * ((xout[1]-xout[3])*(yout[0]-yout[2]) - (xout[0]-xout[2])*(yout[1]-yout[3]));
w = weight_scale / jaco;  // Jacobian-corrected weight
dover = boxer(ii, jj, xout, yout);  // polygon-pixel overlap via sgarea()
```

### Jacobian / pscale_ratio Computation
```c
// Estimated from transformation determinant at polygon centroid
pscale_ratio = sqrt(|cd11*cd22 - cd12*cd21|);
```

## Comparison with Other Tools

| Feature | Us | STScI | Siril | PixInsight |
|---------|-----|-------|-------|------------|
| **Turbo kernel** | Yes | Yes | Yes | No |
| **Point kernel** | Yes | Yes | Yes | No |
| **Gaussian kernel** | Yes (2-pass) | Yes (analytical) | Yes | Yes |
| **Lanczos kernel** | Lanczos-3 | Lanczos-2/3 | Lanczos-2/3 | No |
| **Square kernel** | No | Yes | Yes | Yes |
| **Circular kernel** | No | No | No | Yes |
| **Variable-shape** | No | No | No | Yes |
| **Per-pixel weights** | No | Yes | Flat-based | Via ImageIntegration |
| **Variance output** | No | 3-component | No | No |
| **Context image** | No | Yes (bitmask) | Rejection maps | Rejection maps |
| **Coverage map** | Yes | Yes | No | Yes (normalized) |
| **CFA/Bayer drizzle** | No | N/A | Yes | Yes |
| **Jacobian correction** | No | Yes (per-pixel) | Unknown | Yes (splines) |
| **Scale range** | Any f32 | Continuous | 0.1-3.0 | Integer (2x, 3x) |
| **Parallel accumulation** | No | No | Unknown | Fast Drizzle (Sartori) |

## What's Correct

- **Turbo kernel**: Correct axis-aligned drop, drop_size = pixfrac * scale, inv_area normalization
- **Point kernel**: Correct nearest-pixel contribution at transformed center
- **Gaussian kernel**: Correct FWHM = pixfrac * scale, sigma = FWHM / 2.3548
- **Lanczos kernel**: Correct sinc-windowed Lanczos-3 formula, singularity handling at x=0
- **Rectangle overlap**: Correct AABB intersection (matches STScI `over()` function)
- **Two-pass weighted normalization**: Correct weighted-mean formula (data/weight), algebraically
  equivalent to STScI incremental formula for typical frame counts
- **min_coverage**: Correctly compared against normalized weight (fraction of max_weight)
- **Coverage map**: Per-spatial-pixel (channel 0), correctly normalized to [0, 1]
- **Output dimensions**: Correct: `ceil(input_dim * scale)`
- **Finalize**: Row-parallel via rayon, Lanczos clamping to [0, +inf)

## What Differs from Reference

### Gaussian Normalization Approach
**Our approach**: Two-pass -- first sum all Gaussian weights in support, then distribute
flux proportionally (`pixel_weight = weight * gauss / total_gauss`). Ensures weights sum
to 1.0 per input pixel.

**STScI approach**: Single-pass with analytical normalization constant
`gaussian_es = gaussian_efac / PI`. Does not explicitly normalize to 1.0. Faster (one pass)
but less numerically precise near image borders where support is truncated.

**Impact**: Both produce correct results for interior pixels. Ours is more accurate near
borders (adaptive normalization), STScI is faster. Effectively equivalent for practical use.

### Gaussian Support Radius
**Ours**: `radius = ceil(3 * sigma)` where sigma = drop_size / 2.3548
**STScI**: `pfo = nsig * pixel_fraction / 2.3548 / pscale_ratio` with nsig = 2.5
STScI also has minimum floor: `pfo = max(pfo, 1.2 / pscale_ratio)`.
**Impact**: Our support may be slightly wider (3-sigma vs 2.5-sigma). Minor.

### Turbo Kernel Weight Scale
**Ours**: `pixel_weight = weight * overlap * inv_area` where `inv_area = 1/(drop_size^2)`
**STScI**: `dow = over() * dover_scale * w` where `dover_scale = ac * pscale_ratio^2`
  = `(1/pixfrac^2) * (1/scale)^2` = `1/(pixfrac*scale)^2` = `1/drop_size^2`
**Impact**: Identical formula. Confirmed equivalent.

### No s^2 Surface Brightness Factor
The paper's Equation 5 includes an explicit `s^2` factor (s = output/input pixel size ratio)
to conserve surface brightness for count-based (photon counting) images. STScI implements
this via `iscale` parameter: `d = pixel_value * iscale`. Our code does NOT include `s^2`
because input images are already normalized surface brightness (f32 values), and we divide
by accumulated weights in finalize. This is correct for amateur astrophotography with
normalized images but would be wrong for raw photon-count images.

## Missing Features (Priority Order)

### P1: Per-Pixel Input Weights / Bad Pixel Masks
Only scalar weight per frame. Cannot mask bad/hot/dead pixels, cosmic rays, satellite
trails. STScI, Siril, and PixInsight all accept per-pixel weight maps. Critical for
real-world astrophotography data quality.

### P2: True Square Kernel (Polygon Clipping)
Current Turbo kernel is axis-aligned approximation. True Square kernel transforms all
4 corners and computes polygon-polygon overlap. Implemented in STScI and Siril. Important
when rotation between frames exceeds ~1 degree. Requires Sutherland-Hodgman clipping.

### P2: Jacobian / Geometric Distortion Correction
Constant `drop_size` for all pixels. For non-affine transforms (homography) with spatially
varying magnification, should compute local Jacobian: weight = overlap / jaco. STScI uses
`jaco = 0.5*((x1-x3)*(y0-y2) - (x0-x2)*(y1-y3))` from 4 transformed corners.
Correct for translation/rotation/similarity (constant Jacobian).

### P2: CFA/Bayer Drizzle
Raw CFA pixels used directly without debayering, filling RGB grid via dithering offsets.
Reduces debayering artifacts and improves color rendering. Supported by Siril, PixInsight,
DSS, and APP. Important for OSC camera users.

### P3: Context/Contribution Image
32-bit bitmask of contributing frames per output pixel. Useful for identifying artifacts,
error estimation, and debugging alignment. STScI standard output.

### P3: Variance/Error Output
STScI/JWST propagates 3 variance components (read noise, Poisson, flat) using squared
weights: `var_new = (old_var * w^2 + new_var * dow^2) / (w + dow)^2`. Produces error
image as sqrt(sum of variances). Important for scientific photometry.

### P3: Parallel Accumulation
`add_image_*` loops are single-threaded. Options: per-thread accumulators (2x memory),
atomic operations (slow for f32), or output-pixel-parallel with conflict avoidance.
PixInsight's Fast Drizzle (Sartori, 2024) achieved significant speedup.

### P4: Lanczos Constraint Enforcement
Currently warns but doesn't error on Lanczos with pixfrac != 1.0 or scale != 1.0.
Should assert at config time since output is mathematically invalid.

## Design Notes

### Two-Pass vs Incremental Accumulation
STScI uses single-pass incremental: `I' = (I*W + i*a*w) / (W + a*w)`. We accumulate
`data += flux * weight` and `weights += weight`, then divide. Both are algebraically
equivalent. Two-pass avoids division per-pixel during accumulation (faster inner loop)
and avoids numerical issues when W is very small. For N < ~1000 frames (typical amateur),
no precision difference. The STScI approach uses less memory for variance (no separate
data/weight arrays needed), but we don't implement variance yet.

### Correlated Noise
Drizzle output has correlated noise between adjacent pixels. The noise correlation ratio R
depends on pixfrac (p) and scale (s). For p=0.6, s=0.5: R=1.662. Weight maps should be
used for proper photometric error estimation. Not currently tracked or warned about.

### When NOT to Drizzle
- Images already well-sampled (FWHM > 2-3 pixels): noise penalty without resolution gain
- Fewer than ~15 frames: insufficient dithering coverage
- Without sub-pixel dithering: grid-aligned frames produce no benefit
- Large rotations (>5 degrees) with Turbo kernel: use Square kernel instead

## Dependencies
- `Transform`: 6 types (Translation through Homography), f64 precision `apply(DVec2)`
- `AstroImage`: Planar storage, `channel(c) -> &Buffer2<f32>`, owned consumption
- `Buffer2<T>`: Row-major `y * width + x`, `get_mut(x, y)` for accumulation
- `drizzle_stack` exported from lumos crate but no external callers yet

## References
- Fruchter & Hook 2002, PASP 114:144-152 -- Original drizzle paper (arXiv: astro-ph/9808087)
- Fruchter 2011, PASP 123:497-502 -- iDrizzle: iterative band-limited imaging
- STScI DrizzlePac Handbook -- https://hst-docs.stsci.edu/drizzpac
- STScI drizzle C library (cdrizzlebox.c) -- https://github.com/spacetelescope/drizzle
- STScI DrizzlePac kernel docs -- https://drizzlepac.readthedocs.io/en/latest/drizzlepac_api/adrizzle.html
- JWST Resample step -- https://jwst-pipeline.readthedocs.io/en/stable/jwst/resample/main.html
- Siril drizzle docs -- https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html
- PixInsight DrizzleIntegration -- https://pixinsight.com/forum/index.php?threads/drizzleintegration-kernels.20837/
- PixInsight Fast Drizzle (Sartori 2024) -- https://www.diyphotography.net/pixinsight-1-9-lockhart-released
- DeepSkyStacker -- https://github.com/deepskystacker/DSS
- AstroPixelProcessor -- https://www.astropixelprocessor.com/community/tutorials-workflows/drizzle-for-mono-cameras/
