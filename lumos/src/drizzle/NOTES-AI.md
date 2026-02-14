# drizzle Module - Implementation Notes

## Overview
Implements Variable-Pixel Linear Reconstruction (Drizzle algorithm, Fruchter & Hook 2002,
PASP 114:144). Takes dithered input images with geometric transforms, shrinks input pixels
into "drops" (controlled by pixfrac), maps them onto a higher-resolution output grid
(controlled by scale), and accumulates weighted contributions. Supports four kernels:
Turbo, Point, Gaussian, Lanczos.

## Architecture
- `DrizzleAccumulator`: Accumulate contributions from multiple frames, then finalize
  - `data: ArrayVec<Buffer2<f32>, 3>` — per-channel accumulated weighted flux
  - `weights: ArrayVec<Buffer2<f32>, 3>` — per-channel accumulated weights
  - `new(input_dims: ImageDimensions, config)` / `dimensions() -> ImageDimensions`
- `DrizzleResult.coverage: Buffer2<f32>` — normalized [0,1] coverage map
- `add_image()` consumes `AstroImage` (owned); kernel methods borrow `&AstroImage`
- No interleaved allocation — kernel inner loops use `image.channel(c)[(ix, iy)]` directly
- Axis-aligned rectangle overlap computation for weighted accumulation (Turbo kernel)
- Transform via `Transform::apply(DVec2) -> DVec2` (projective, f64 precision)
- Builder pattern for configuration with validation
- `drizzle_stack()`: High-level API that loads images, applies transforms, and finalizes
- Rayon-parallel finalization (row-parallel normalization and coverage computation)
- Output built via `AstroImage::from_planar_channels()` (no interleaving step)

## Kernels

### Turbo (default)
Axis-aligned rectangular drop centered on the transformed pixel center. Transforms only
the center point, creates axis-aligned bounding box of size `drop_size` in output space.
Fast and adequate when rotation between frames is small. Named "turbo" per STScI DrizzlePac
convention — STScI's "square" kernel uses full polygon clipping (not implemented here).

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
**Only valid at pixfrac=1.0, scale=1.0** — a runtime warning is emitted if these
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
finalize compute `output = data / weight`. Algebraically equivalent to STScI incremental
formula for the expected frame counts in amateur astrophotography (tens to hundreds).

### min_coverage
Compared against normalized weight: `weight_threshold = min_coverage * max_weight`.
Coverage map uses channel 0 weight as representative (all channels have identical
geometric overlap). Normalized to [0, 1] in the output.

### Finalize
- Row-parallel via rayon (`par_chunks_mut`)
- Lanczos output clamped to `[0, +inf)` to suppress negative ringing
- Coverage computed from channel 0 weight (per-spatial-pixel, not per-channel average)

## Reference Standard: STScI Kernel Implementations (cdrizzlebox.c)

### Square Kernel (true drizzle — NOT implemented)
The original, mathematically exact kernel:
1. Define 4 corners at `center +/- 0.5 * pixfrac` in **input** coordinates
2. Transform ALL 4 corners through the geometric mapping to output coordinates
3. Compute polygon-polygon overlap using `boxer()` + `sgarea()` (Sutherland-Hodgman style)
4. Compute per-pixel Jacobian from the 4 transformed corners
5. Weight = `weight_scale / jaco`

### Turbo Kernel (what this codebase implements as default)
STScI code:
```c
pfo = pixel_fraction / pscale_ratio / 2.0;   // half-width in output pixels
ac = 1.0 / (pixel_fraction * pixel_fraction); // inverse drop area in input pixel units
```

### Gaussian Kernel
STScI code:
```c
pfo = nsig * pixel_fraction / 2.3548 / pscale_ratio;  // nsig = 2.5
gaussian_efac = (2.3548 * 2.3548) * kscale2 * ac / 2.0;
gaussian_es = gaussian_efac / M_PI;
```

### Lanczos Kernel
STScI uses LUT-based Lanczos-2 or Lanczos-3. "Should never be used for pixfrac != 1.0,
and is not recommended for scale != 1.0."

## Remaining Issues (Priority Order)

### Moderate: True Square Kernel Not Implemented
Current "Turbo" kernel is axis-aligned approximation. True Square kernel would transform
all 4 corners and compute polygon-polygon overlap (Sutherland-Hodgman clipping). Important
when rotation between frames is significant (>1 degree).

### Moderate: No Context/Contribution Image
STScI produces a 32-bit bitmask of contributing frames per output pixel. Useful for
identifying artifacts, error estimation, and debugging alignment.

### Moderate: No Per-Pixel Input Weights / Bad Pixel Masks
Only scalar weight per frame. Cannot mask bad/hot/dead pixels, cosmic rays, satellite
trails. STScI accepts per-pixel weight maps (inverse variance maps).

### Moderate: No Jacobian / Geometric Distortion Correction
Constant `drop_size` for all pixels. For non-affine transforms with spatially varying
magnification, should compute local Jacobian from 4 transformed corners. Correct for
translation/rotation/similarity transforms (constant Jacobian).

### Moderate: No Variance/Error Output
JWST pipeline propagates read noise, Poisson noise, and flat-field variance through
drizzle. Not implemented.

### Minor: Accumulation Loops Are Single-Threaded
`add_image_*` loops iterate over input pixels writing to overlapping output regions.
Parallelizing requires per-thread accumulators (memory cost) or atomic operations.
Finalization is already parallelized.

## Correct Implementations
- **Turbo kernel:** Correct axis-aligned drop with `pixfrac * scale` drop size
- **Point kernel:** Correct single-pixel contribution at transformed center
- **Gaussian kernel:** Correct FWHM = `pixfrac * scale`, sigma = FWHM / 2.3548
- **Lanczos kernel:** Correct sinc-windowed formula, support radius = 3, with constraint
  validation and output clamping
- **Lanczos kernel function:** Correct sinc-windowed formula with singularity handling at x=0
- **Rectangle overlap (`compute_square_overlap`):** Correct AABB intersection
- **Projective transform:** Uses `Transform::apply(DVec2)` with f64 precision and perspective division
- **Two-pass weighted normalization:** Correct weighted-mean formula (data/weight)
- **min_coverage:** Correctly compared against normalized weight (fraction of max_weight)
- **Coverage map:** Per-spatial-pixel (channel 0), not averaged across channels
- **Builder pattern config:** Correct with pixfrac/min_coverage range validation
- **Output dimension calculation:** Correct: `ceil(input_dim * scale)`
- **Finalize:** Row-parallel via rayon, Lanczos clamping to [0, +inf)

## References
- Fruchter & Hook 2002, PASP 114:144-152 — Original drizzle paper (arXiv: astro-ph/9808087)
- STScI DrizzlePac Handbook — https://hst-docs.stsci.edu/drizzpac
- STScI drizzle C library (cdrizzlebox.c) — https://github.com/spacetelescope/drizzle
- STScI DrizzlePac kernel docs — https://drizzlepac.readthedocs.io/en/latest/drizzlepac_api/adrizzle.html
- Siril drizzle docs — https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html
- Starlink DRIZZLE docs — https://starlink.eao.hawaii.edu/docs/sun139.htx/sun139ss14.html
- JWST Resample step — https://jwst-pipeline.readthedocs.io/en/stable/jwst/resample/main.html
- PixInsight PCL DrizzleIntegration — https://github.com/PixInsight/PCL
