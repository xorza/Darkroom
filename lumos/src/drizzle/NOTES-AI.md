# drizzle Module - Implementation Notes

## Module Overview

Single-file module (`mod.rs`, ~1450 lines incl. tests) implementing Variable-Pixel Linear
Reconstruction (the Drizzle algorithm, Fruchter & Hook 2002, PASP 114:144-152). Takes
dithered input images with geometric transforms, shrinks input pixels into "drops" (controlled
by `pixfrac`), maps them onto a higher-resolution output grid (controlled by `scale`), and
accumulates weighted contributions.

### Architecture

- `DrizzleConfig`: Builder-pattern configuration (scale, pixfrac, kernel, fill_value, min_coverage)
- `DrizzleAccumulator`: Core accumulator with per-channel `data` and `weights` buffers
  - `data: ArrayVec<Buffer2<f32>, 3>` -- accumulated `flux * weight` per channel
  - `weights: ArrayVec<Buffer2<f32>, 3>` -- accumulated weight per channel
  - `add_image(image, transform, weight, pixel_weights)` -- dispatches to kernel-specific method
  - `pixel_weights: Option<&Buffer2<f32>>` -- per-pixel weight map (0=exclude, 1=normal)
  - `accumulate()` -- shared inline helper iterating over channels
  - `finalize()` -- normalizes `data / weights`, applies min_coverage, returns DrizzleResult
- `DrizzleResult`: Final image + normalized [0,1] coverage map (channel 0 weights)
- `drizzle_stack()`: High-level API loading images from paths, sequential accumulation
- `add_image_radial()`: Unified two-pass Gaussian/Lanczos via closure-based `kernel_fn`
- Transform via `Transform::apply(DVec2) -> DVec2` (f64 precision, supports up to Homography)
- Rayon-parallel finalization only; accumulation is single-threaded
- Output built via `AstroImage::from_planar_channels()` (planar, no interleaving)

### Kernels

| Kernel   | Method              | Drop Shape             | Complexity per input pixel |
|----------|---------------------|------------------------|---------------------------|
| Square   | `add_image_square`  | True quadrilateral     | O(bbox) polygon clipping  |
| Turbo    | `add_image_turbo`   | Axis-aligned rectangle | O(drop_size^2) overlaps   |
| Point    | `add_image_point`   | Delta function         | O(1) -- single pixel      |
| Gaussian | `add_image_radial`  | Gaussian bell          | O((3*sigma)^2) two-pass   |
| Lanczos  | `add_image_radial`  | Lanczos-3 sinc window  | O(49) two-pass (7x7)      |

---

## Algorithm Reference (Fruchter & Hook 2002)

### Core Equations

The paper defines these key quantities (Equations 3-5):

**Drop overlap weight** (Eq. 3-4):
```
a_xy = overlap_area(drop, output_pixel) / drop_area
```
Each input pixel `i(x,y)` with per-pixel weight `w(x,y)` contributes to output pixel `I(x,y)`:

**Incremental update** (STScI `update_data_var`):
```
I'(x,y) = (I(x,y) * W(x,y) + s^2 * i * a * w) / (W(x,y) + a * w)
W'(x,y) = W(x,y) + a * w
```

Where:
- `s` = output pixel size / input pixel size ratio = `1/scale` (our convention: `s = pscale_ratio`)
- `p` = pixfrac (drop linear fraction of input pixel)
- `a` = fractional overlap area between the drop and the output pixel
- `w` = per-pixel input weight (exposure time * inverse variance)
- `s^2` factor converts counts to surface brightness (for photon-counting detectors)

**Drop size in output pixels**: `drop = p / s = pixfrac * scale`

**This codebase uses two-pass accumulation** (algebraically equivalent for typical frame counts):
```
data  += flux * pixel_weight
weight += pixel_weight
output = data / weight       (in finalize)
```

### Convention Mapping

| Paper / STScI C code         | This codebase       | Relationship              |
|------------------------------|---------------------|---------------------------|
| `s` (pscale_ratio)           | `1.0 / scale`       | Reciprocal                |
| `p` (pixel_fraction)         | `pixfrac`           | Same                      |
| Drop in output px: `p/s`     | `pixfrac * scale`   | Same formula              |
| `pfo` (half-width)           | `half_drop`         | `pixfrac * scale / 2`    |
| `ac` = `1/p^2`               | `inv_area`          | `1 / drop_size^2`        |
| `dover_scale` = `ac * s^2`   | `inv_area`          | Same (since `ac*s^2 = 1/(p*1/s)^2 = 1/drop_size^2`) |
| `iscale` = `s^2`             | Not applied          | See s^2 discussion below  |

---

## Industry Standard Comparison

### STScI cdrizzle (Reference Implementation)

Source: `cdrizzlebox.c` in [spacetelescope/drizzle](https://github.com/spacetelescope/drizzle)

**Turbo kernel** (`do_kernel_turbo`):
```c
pfo = pixel_fraction / pscale_ratio / 2.0;    // half-width in output pixels
ac  = 1.0 / (pixel_fraction * pixel_fraction); // inverse drop area (input units)
dover_scale = ac * pscale_ratio * pscale_ratio; // = 1/(pixfrac*scale)^2
dow = over(ii, jj, xxi, xxa, yyi, yya) * dover_scale * w;
```
The `over()` function computes axis-aligned rectangle overlap:
```c
dx = MIN(xmax, i + 0.5) - MAX(xmin, i - 0.5);
dy = MIN(ymax, j + 0.5) - MAX(ymin, j - 0.5);
return (dx > 0 && dy > 0) ? dx * dy : 0.0;
```

**Square kernel** (`do_kernel_square`) -- ported to `add_image_square`:
```c
// Transform ALL 4 corners through geometric mapping
interpolate_four_points(p, i, j, xin, yin, xout, yout);
jaco = 0.5 * ((xout[1]-xout[3])*(yout[0]-yout[2]) - (xout[0]-xout[2])*(yout[1]-yout[3]));
w = weight_scale / jaco;     // Jacobian-corrected weight
dover = boxer(ii, jj, xout, yout);  // polygon-pixel overlap via sgarea()
```
The `boxer()` function computes exact quadrilateral-to-unit-square overlap using
`sgarea()` (signed area under line segments clipped to the unit square). This is
Sutherland-Hodgman-style polygon clipping -- NOT simple AABB intersection.

**Gaussian kernel** (`do_kernel_gaussian`):
```c
pfo = nsig * pixel_fraction / 2.3548 / pscale_ratio;  // nsig = 2.5
gaussian_efac = (2.3548*2.3548) * kscale2 * ac / 2.0;
gaussian_es = gaussian_efac / M_PI;  // analytical normalization constant
dover = gaussian_es * exp(-r2 * gaussian_efac);  // single-pass, NO normalization loop
```

**Lanczos kernel** (`do_kernel_lanczos`):
```c
pfo = kernel_order / pscale_ratio;  // order = 2 or 3
// Uses precomputed LUT with delta = 0.003
dover = lut[ix] * lut[iy];  // separable product
```

**Point kernel** (`do_kernel_point`):
```c
ii = nintd(ox);  jj = nintd(oy);  // nearest output pixel
dow = get_pixel(weights, i, j) * weight_scale;
```

**Core accumulation** (`update_data_var`):
```c
// Incremental weighted average:
value = (output_data[ii,jj] * vc + dow * d) / (vc + dow);
// Variance propagation (3 components):
var_new = (old_var * vc^2 + dow^2 * new_var) / (vc + dow)^2;
// DQ bitmask:
output_dq |= input_dq;
```

### Siril (v1.4+)

Source: [GitLab free-astro/siril](https://gitlab.com/free-astro/siril)

- Replaced simplified drizzle with full HST-style algorithm in v1.4.0
- Supports 6 kernels: point, turbo, **square** (default), gaussian, lanczos2, lanczos3
- Square kernel is default; uses polygon clipping like STScI
- CFA/Bayer drizzle: routes raw CFA pixel to appropriate RGB channel based on CFA position
- Per-pixel weighting via master flat
- Lanczos restricted to scale=1, pixfrac=1 (same as STScI)
- Found and fixed a bug in the original STScI drizzle code during development

### PixInsight (v1.9 Lockhart)

Source: [PCL DrizzleIntegrationInstance.cpp](https://github.com/PixInsight/PCL)

- Kernels: **square** (default), circular, gaussian, variable-shape
- No turbo, no point, no Lanczos
- Variable-shape kernel: constant shape per image, kurtosis parameter varies global shape
- Gaussian/variable-shape: grid-based numerical integration (default 16x16 = 256 samples)
- CFA/Bayer drizzle: BayerDrizzlePrep script + DrizzleIntegration
- "Fast Drizzle" algorithm by Roberto Sartori (2024): significantly faster than classical drizzle
  with parallel execution; enables practical use of circular/gaussian/variable-shape kernels
- Drop shrink factor = pixfrac (default 0.9, range 0.7-1.0 typical)
- Integer scale factors only (2x, 3x) in typical usage
- Adaptive thread execution optimization (v1.9.3)

### Feature Comparison

| Feature                      | This Code      | STScI          | Siril          | PixInsight     |
|------------------------------|----------------|----------------|----------------|----------------|
| **Turbo kernel**             | Yes            | Yes            | Yes            | No             |
| **Point kernel**             | Yes            | Yes            | Yes            | No             |
| **Gaussian kernel**          | Yes (2-pass)   | Yes (analytic) | Yes            | Yes (grid)     |
| **Lanczos kernel**           | Lanczos-3      | Lanczos-2/3    | Lanczos-2/3    | No             |
| **Square kernel (polygon)**  | Yes            | Yes            | Yes (default)  | Yes (default)  |
| **Circular kernel**          | No             | No             | No             | Yes            |
| **Variable-shape kernel**    | No             | No             | No             | Yes            |
| **Per-pixel input weights**  | Yes            | Yes            | Flat-based     | Via II process |
| **Variance/error output**    | **No**         | 3-component    | No             | No             |
| **Context/contribution map** | No             | Yes (bitmask)  | Rejection maps | Rejection maps |
| **Coverage map**             | Yes            | Yes            | No             | Yes            |
| **CFA/Bayer drizzle**        | **No**         | N/A (HST)      | Yes            | Yes            |
| **Jacobian correction**      | All kernels    | Yes (per-pixel)| Yes            | Yes (splines)  |
| **Parallel accumulation**    | **No**         | No             | Unknown        | Fast Drizzle   |
| **Scale range**              | Any f32        | Continuous     | 0.1-3.0        | Typically int  |

---

## Kernel Correctness Analysis

### Square Kernel -- CORRECT

Ported from STScI `do_kernel_square`, `boxer()`, and `sgarea()`:

- `sgarea(x1, y1, x2, y2) -> f64`: Signed area between line segment and x-axis, clipped to
  unit square [0,1]×[0,1]. Uses Green's theorem. Three cases: trapezoid (both y in [0,1]),
  enters-inside-exits-above, enters-above-exits-inside. ~40 lines, all f64.

- `boxer(ox, oy, x[4], y[4]) -> f64`: Shifts quadrilateral so output pixel becomes unit
  square, sums 4 `sgarea()` calls, returns absolute overlap area. Adapted from STScI
  pixel-centered convention (pixel `(i,j)` spans `[i-0.5, i+0.5]`) to our pixel-corner
  convention (pixel `(ox, oy)` spans `[ox, ox+1]`).

- `add_image_square()`: For each input pixel:
  1. Compute 4 corners of shrunken drop: `center ± 0.5*pixfrac` (BL, BR, TR, TL winding)
  2. Transform all 4 corners via `transform.apply()` and scale to output coords
  3. Compute Jacobian: `0.5 * ((x1-x3)*(y0-y2) - (x0-x2)*(y1-y3))` (signed area)
  4. Iterate bounding box, call `boxer()` per output pixel
  5. Weight = `overlap * effective_weight / abs_jaco` (Jacobian-corrected)

**Verified against Turbo kernel**: For pure translation (no rotation), Square produces
identical output to Turbo (< 1e-3 error). For identity + scale, exact pixel values match
hand-computed overlap/Jacobian ratios.

**Key difference from Turbo**: Turbo transforms only the center point and constructs an
axis-aligned box. Square transforms all 4 corners, so the output quadrilateral correctly
reflects rotation and shear. Weight includes Jacobian correction for area distortion.

### Turbo Kernel -- CORRECT

Matches STScI `do_kernel_turbo` exactly:
- Transforms only center point: `transform.apply(DVec2(ix+0.5, iy+0.5))`
- Drop is axis-aligned rectangle of size `drop_size = pixfrac * scale` output pixels
- Overlap via `compute_square_overlap()` -- identical to STScI `over()` function
- Weight: `overlap * inv_area` where `inv_area = 1/drop_size^2` = STScI `dover_scale`
- Correctly ignores rotation (by design -- that is the turbo approximation)

**Verified**: `dover_scale = ac * pscale_ratio^2 = (1/p^2) * (1/scale)^2 = 1/(p*scale)^2 = inv_area`

### Point Kernel -- CORRECT

Matches STScI `do_kernel_point`:
- Maps center to `floor(transformed * scale)` -- nearest output pixel
- Full frame weight applied to single pixel
- Correctly checks bounds before accumulation

### Gaussian Kernel -- CORRECT (different approach, same result)

**Difference from STScI**: Two-pass normalization vs analytical constant.

Our approach:
```rust
// Pass 1: total_weight = sum(gauss(dx, dy)) over support
// Pass 2: pixel_weight = weight * gauss(dx, dy) / total_weight
```

STScI approach:
```c
gaussian_es = gaussian_efac / M_PI;  // analytical normalization
dover = gaussian_es * exp(-r2 * gaussian_efac);  // no normalization loop
```

**Analysis**: Both are correct. Our approach is more accurate near image borders where the
Gaussian support is clipped (adaptive normalization ensures weights sum to 1.0 even with
truncated support). STScI is faster (single pass) but allows small flux leaks at borders.
For interior pixels, results are identical to machine precision.

**Support radius difference**: Our `ceil(3 * sigma)` vs STScI `nsig=2.5` sigma. Our support
is ~20% wider, which captures more of the Gaussian tail (99.7% vs 98.8%). Negligible impact.

**FWHM parameter**: Both use `FWHM = pixfrac * scale` (in output pixels), converted via
`sigma = FWHM / 2.3548`. Correct.

### Lanczos Kernel -- CORRECT (with appropriate constraints)

Matches STScI `do_kernel_lanczos` with separable Lanczos-3:
- `lanczos_kernel(x, a)` = `sinc(x) * sinc(x/a)` for |x| < a, else 0
- Singularity at x=0 handled: returns 1.0 (correct limit)
- Symmetry: f(x) = f(-x) -- verified in tests
- Support: radius = 3 pixels (Lanczos-3, `a = 3.0`)
- Two-pass normalization ensures sum of weights = 1.0 (same approach as Gaussian)
- Output clamped to [0, +inf) in `finalize()` to suppress negative ringing

**STScI uses LUT** (`lut_delta = 0.003`) instead of computing sin/cos per pixel. Our approach
computes the kernel analytically, which is slower but more precise. For typical Lanczos use
(scale=1, pixfrac=1), the 7x7 kernel per pixel makes this a minor difference.

**Warning vs assertion**: Our code warns on Lanczos with pixfrac!=1.0 or scale!=1.0 but
does not prevent it. STScI docs state it "should never be used" outside these constraints.

### Rectangle Overlap (`compute_square_overlap`) -- CORRECT

Exact match to STScI `over()`:
```rust
// Ours:
x_overlap = min(ax2, bx2) - max(ax1, bx1)  // clamped to 0
y_overlap = min(ay2, by2) - max(ay1, by1)
area = x_overlap * y_overlap

// STScI:
dx = MIN(xmax, i + 0.5) - MAX(xmin, i - 0.5)
dy = MIN(ymax, j + 0.5) - MAX(ymin, j - 0.5)
area = dx * dy
```
Our function takes explicit rectangle bounds; STScI `over()` takes pixel index and implicitly
constructs [i-0.5, i+0.5] x [j-0.5, j+0.5]. Mathematically identical.

### Weight Accumulation -- CORRECT

Two-pass accumulation (`data += flux * w; weights += w; output = data / weights`) is
algebraically equivalent to STScI's incremental formula for any number of frames:
```
STScI: I' = (I*W + d*dow) / (W + dow)
Ours:  I  = sum(d_k * dow_k) / sum(dow_k)
```
Both compute the weighted mean. Our approach avoids division per pixel during accumulation
(slightly faster inner loop) and avoids numerical issues when W is very small in early frames.

### The s^2 Factor -- INTENTIONALLY OMITTED (correct for our use case)

The paper's Equation 5 includes `s^2` (`iscale` in STScI code) to convert photon counts to
surface brightness when changing pixel scale. Our images are pre-normalized floating-point
surface brightness values (not raw photon counts), so dividing by accumulated weights in
`finalize()` already preserves surface brightness correctly. This is standard practice in
amateur astrophotography software (Siril, DeepSkyStacker, etc.).

---

## Missing Features (with severity)

### ~~P1 (Critical): Per-Pixel Input Weights / Bad Pixel Masks~~ -- DONE

Implemented via `pixel_weights: Option<&Buffer2<f32>>` parameter on `add_image()` and
`drizzle_stack()`. Per-pixel weight multiplies into the effective frame weight before
kernel accumulation. Weight of 0.0 fully excludes a pixel (early-out skip in all kernels);
1.0 is normal; intermediate values allow soft weighting. For radial kernels (Gaussian,
Lanczos), the per-pixel weight scales the frame weight, not the kernel shape — kernel
normalization remains purely geometric.

### ~~P2 (Important): True Square Kernel (Polygon Clipping)~~ -- DONE

Implemented as `DrizzleKernel::Square` with `add_image_square()`. Ported STScI `sgarea()`
and `boxer()` functions. Transforms all 4 corners per input pixel, computes exact
quadrilateral-to-pixel overlap via polygon clipping, with Jacobian correction for weight.
Correct for arbitrary transforms including rotation and shear. Also subsumes Jacobian
correction (below) for the Square kernel path.

### ~~P2 (Important): Jacobian / Geometric Distortion Correction~~ -- DONE (all kernels)

All kernels now include Jacobian correction for non-affine transforms:

- **Square**: Computes Jacobian from 4 transformed corners (cross-product of diagonals).
- **Turbo, Point, Gaussian, Lanczos**: Use `local_jacobian()` helper — finite-difference
  approximation of `|det(J)| * scale²` via 2 extra `transform.apply()` calls per input pixel
  (center+dx, center+dy). Reuses the already-computed center transform result.

The `local_jacobian()` function:
```rust
fn local_jacobian(transform, center, ix, iy, scale) -> f64:
    right = transform.apply(ix + 1.5, iy + 0.5)
    down  = transform.apply(ix + 0.5, iy + 1.5)
    dx = right - center;  dy = down - center
    |dx.x * dy.y - dx.y * dy.x| * scale²
```

For affine transforms, `local_jacobian()` returns the same constant `|det(M)| * scale²`
everywhere (mathematically exact). For homographies, it varies spatially — edge pixels may
have 10-20% different magnification than center pixels. Without correction, this creates
subtle photometric gradients across the output. The STScI reference only applies Jacobian
to the Square kernel; our implementation goes beyond by correcting all kernels.

### P2 (Important): CFA/Bayer Drizzle

**What**: Route raw CFA pixels directly to the appropriate RGB channel in the output based
on the Bayer/X-Trans CFA pattern position, bypassing demosaicing entirely.

**Impact**: Eliminates demosaicing artifacts, improves color accuracy, particularly valuable
for one-shot-color (OSC) camera users. Supported by Siril (with scale=1 pixfrac=1
recommended), PixInsight (BayerDrizzlePrep), DeepSkyStacker, and AstroPixelProcessor.

**Implementation**: Add CFA pattern parameter to `add_image()`. Map each raw pixel's CFA
color to the corresponding output channel index. Requires CFA pattern knowledge from the
image metadata.

### P3 (Nice-to-have): Context / Contribution Image

**What**: 32-bit bitmask (or count) per output pixel recording which input frames contributed.

**Impact**: Useful for identifying artifacts, estimating errors, debugging alignment issues.
Standard STScI output. Less critical for amateur astrophotography.

**Implementation**: Add `context: Buffer2<u32>` to accumulator. Set bit `i` when frame `i`
contributes. Return in `DrizzleResult`.

### P3 (Nice-to-have): Variance / Error Output

**What**: Propagate variance through the drizzle weighted averaging using squared weights.

**STScI formula**:
```c
var_new = (old_var * vc^2 + dow^2 * input_var) / (vc + dow)^2
```

**Impact**: Required for scientific photometry with proper error bars. Not typically used in
amateur astrophotography workflows. Would require switching to incremental accumulation
(STScI style) or storing additional variance buffers.

### P3 (Nice-to-have): Parallel Accumulation

**What**: Currently `add_image_*` loops over all input pixels sequentially.

**Impact**: For large images (6000x4000 with scale=2 -> 12000x8000 output), the inner loop
processes 24M input pixels per frame. With turbo kernel touching ~4 output pixels each, this
is ~96M accumulations per frame. At ~10ns each, ~1 second per frame -- acceptable for
tens of frames but slow for hundreds.

**Options** (ordered by implementation ease):
1. **Row-parallel input**: Process input rows in parallel, each row writes to different output
   rows (slight overlap at drop boundaries needs synchronization)
2. **Per-thread accumulators**: Clone accumulator per thread, merge after each frame.
   2x memory but trivially correct. Best for small frame counts.
3. **Tile-based parallelism**: Divide output into tiles, process input pixels per tile.
   Complex but optimal for large images.
4. **Output-pixel parallel** (PixInsight Fast Drizzle approach): For each output pixel,
   find all contributing input pixels. Inverts the loop direction. Natural parallelism
   but requires inverse transform and spatial indexing.

### P4 (Minor): Lanczos Constraint Enforcement

**What**: Currently warns but allows Lanczos with pixfrac != 1.0 or scale != 1.0.

**Impact**: Output is mathematically questionable but not catastrophically wrong. STScI docs:
"should never be used for pixfrac != 1.0, and is not recommended for scale != 1.0."

**Implementation**: Change `tracing::warn!` to `assert!` in `add_image()`, or make it a
configuration-time validation in `DrizzleConfig::with_kernel()`.

---

## Performance Opportunities

### SIMD Vectorization (Turbo Kernel)

The turbo kernel inner loop iterates over output pixels within the drop footprint, computing
axis-aligned rectangle overlap and accumulating weighted flux. For each input pixel:

```rust
for oy in oy_min..oy_max {
    for ox in ox_min..ox_max {
        let overlap = compute_square_overlap(...);
        if overlap > 0.0 {
            pixel_weight = weight * overlap * inv_area;
            accumulate(image, ix, iy, ox, oy, pixel_weight);
        }
    }
}
```

**Opportunity**: The typical drop (pixfrac=0.8, scale=2) spans ~2x2 output pixels. The
overlap computation is 4 min/max + 2 multiplies -- very cheap scalar. SIMD gains would come
from processing multiple input pixels simultaneously (vectorizing the outer `ix` loop), but
the variable output footprint and scattered writes make this challenging.

**Better target**: Vectorize the `accumulate()` function for multi-channel (RGB) images.
Currently iterates channels with `zip`. For 3 channels, could use SSE to process all 3
simultaneously (load flux[3], multiply by weight, add to data[3], add weight to weights[3]).

### Row-Parallel Accumulation

The simplest performance win: parallelize the outer `iy` loop with rayon. Each input row's
drops typically land on different output rows (at most overlapping by `drop_size` rows with
neighboring input rows).

**Approach**: Use `par_iter()` over input rows. Each thread needs its own mutable accumulator
slice, or use atomic f32 (available via `AtomicU32` with `f32::to_bits/from_bits` CAS loops).
Alternatively, partition output rows into non-overlapping bands and process corresponding
input rows in parallel.

**Estimated speedup**: Near-linear with core count for the accumulation phase. Currently
accumulation is 100% of `add_image` time.

### LUT for Lanczos Kernel

STScI uses a precomputed lookup table (`lut_delta = 0.003`) for Lanczos kernel values.
Our implementation computes `sin()` analytically per pixel. For Lanczos-3 with radius=3,
each input pixel touches up to 49 output pixels, requiring 14 sin() calls (7 x-values + 7
y-values, separable). A LUT would reduce this to 14 table lookups.

**Estimated speedup**: ~3-5x for Lanczos kernel specifically (sin() is ~20 cycles, table
lookup is ~4 cycles with L1 hit).

### Finalization Already Parallel

The `finalize()` method uses `par_chunks_mut` for row-parallel normalization and
`par_iter_mut` for coverage normalization. This is already optimal.

---

## Recommendations

### Short-term (correctness/quality)

1. **Enforce Lanczos constraints** (P4) -- trivial change, prevents user confusion.

2. **Add Lanczos-2 option** -- simple parameter change (`a = 2.0` instead of `a = 3.0`),
   matches Siril/STScI option set. Lanczos-2 is faster (5x5 vs 7x7) and often sufficient.

### Medium-term (feature parity)

3. **Row-parallel accumulation** (P3) -- biggest performance win with minimal complexity.
   Use rayon `par_iter()` over input rows with atomic accumulation or band partitioning.

### Long-term (advanced)

4. **CFA/Bayer drizzle** (P2) -- significant feature for OSC camera users.

5. **Variance propagation** (P3) -- needed for scientific photometry. May require switching
   to incremental accumulation (STScI style) for numerical stability with variance.

6. **SIMD for multi-channel accumulate** -- minor gain for RGB, bigger gain if extended to
   process multiple input pixels per iteration.

---

## Design Notes

### Two-Pass vs Incremental Accumulation

STScI uses single-pass incremental: `I' = (I*W + i*dow) / (W + dow)`. We accumulate
`data += flux * weight` and `weights += weight`, then divide in `finalize()`.

Both are algebraically equivalent. Our approach:
- Avoids division per-pixel during accumulation (faster inner loop)
- Avoids numerical issues when W is very small (early frames)
- Requires 2x memory (separate data + weights buffers)
- Cannot do incremental variance propagation (would need STScI approach for that)

For N < ~1000 frames (typical amateur astrophotography), there is no precision difference.

### Correlated Noise

Drizzle output has correlated noise between adjacent pixels. The noise correlation depends
on pixfrac and scale. Smaller pixfrac reduces correlation but increases noise. Weight maps
should be used for proper photometric error estimation, not simple pixel statistics.

Key relationships:
- `pixfrac=1.0`: maximum correlation (shift-and-add equivalent)
- `pixfrac->0`: minimum correlation but requires perfect dithering coverage
- `pixfrac=0.8, scale=2`: reasonable balance for 4+ frame dithers
- Fewer than ~15 frames with small pixfrac risks "dry" (uncovered) output pixels

### When NOT to Drizzle

- Images already well-sampled (FWHM > 2-3 pixels): noise penalty without resolution gain
- Fewer than ~15 frames: insufficient dithering coverage for small pixfrac
- Without sub-pixel dithering: grid-aligned frames produce checkerboard artifacts
- Large rotations (>5 degrees) with Turbo kernel: use Square kernel instead

---

## Dependencies

- `Transform`: 6 types (Translation, Euclidean, Similarity, Affine, Homography, Auto),
  f64 precision `apply(DVec2) -> DVec2`. Homography includes perspective division.
- `AstroImage`: Planar f32 storage, `channel(c) -> &Buffer2<f32>`, `from_planar_channels()`
- `Buffer2<T>`: Row-major `[(x, y)]` indexing = `y * width + x`, `get_mut(x, y)` returns `&mut T`
- `ArrayVec<Buffer2<f32>, 3>`: Fixed-capacity per-channel storage (max RGB = 3)
- `rayon`: Row-parallel finalization
- `drizzle_stack()` is the public API; `DrizzleAccumulator` is also public for manual use

## References

- Fruchter & Hook 2002, PASP 114:144-152 -- [Original drizzle paper](https://arxiv.org/abs/astro-ph/9808087)
- Fruchter 2011, PASP 123:497-502 -- iDrizzle: iterative band-limited imaging
- STScI DrizzlePac Handbook -- https://hst-docs.stsci.edu/drizzpac
- STScI drizzle C library (cdrizzlebox.c) -- https://github.com/spacetelescope/drizzle
- STScI DrizzlePac kernel docs -- https://drizzlepac.readthedocs.io/en/latest/drizzlepac_api/adrizzle.html
- JWST Resample step -- https://jwst-pipeline.readthedocs.io/en/stable/jwst/resample/main.html
- Siril drizzle docs -- https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html
- Siril GitLab -- https://gitlab.com/free-astro/siril
- PixInsight DrizzleIntegration kernels -- https://pixinsight.com/forum/index.php?threads/drizzleintegration-kernels.20837/
- PixInsight PCL source -- https://github.com/PixInsight/PCL
- PixInsight Fast Drizzle (Sartori 2024) -- https://www.diyphotography.net/pixinsight-1-9-lockhart-released
- DeepSkyStacker -- https://github.com/deepskystacker/DSS
- AstroPixelProcessor -- https://www.astropixelprocessor.com/community/tutorials-workflows/drizzle-for-mono-cameras/
- Casertano et al. 2000, AJ 120:2747 -- Noise properties of drizzled images
- Drizzle: A Gentle Introduction -- https://every-algorithm.github.io/2025/01/06/drizzle.html
