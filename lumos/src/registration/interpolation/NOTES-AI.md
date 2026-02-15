# Interpolation / Warp Module

## Architecture Overview

This module provides image interpolation and warping for astronomical image registration.
It supports six interpolation methods: Nearest, Bilinear, Bicubic (Catmull-Rom a=-0.5),
Lanczos2, Lanczos3 (default), and Lanczos4. Warping is performed via `warp_image()` which
parallelizes row processing with rayon. Bilinear has a dedicated SIMD path; all three
Lanczos kernels (2/3/4) share a single const-generic optimized path with incremental
coordinate stepping, `lookup_positive()`, interior fast-path, and deringing support.
Nearest and Bicubic go through a generic per-pixel loop with incremental stepping.

**Files:**
- `mod.rs` -- Kernel functions, LUT, per-pixel interpolation, `warp_image` dispatcher
- `warp/mod.rs` -- Row-level warping: bilinear scalar, Lanczos generic optimized, SIMD dispatch
- `warp/sse.rs` -- AVX2/SSE4.1 SIMD bilinear + SSE FMA Lanczos kernel (generic for all sizes)
- `tests.rs` -- Unit and quality tests for kernels, interpolation, and warping
- `bench.rs` -- Benchmarks for 1k/2k/4k Lanczos3 warps, bilinear, LUT lookup

**Data flow:**
1. `warp_image()` dispatches per-row via rayon `par_chunks_mut`
2. For Bilinear: `warp_row_bilinear()` -> SIMD (AVX2/SSE4.1) or scalar
3. For Lanczos2/3/4: `warp_row_lanczos()` -> const-generic `<A, SIZE, DERINGING>` dispatch ->
   SIMD FMA kernel (all sizes, interior) or scalar fast path (interior) or slow path (border)
4. For Nearest/Bicubic: generic per-pixel `interpolate()` loop with incremental stepping
5. All paths use inverse mapping: output pixel -> transform -> sample input

**Key types:**
- `WarpParams`: bundles `InterpolationMethod` + `border_value`
- `WarpTransform`: `Transform` + optional `SipPolynomial` for nonlinear distortion
- `LanczosLut`: pre-computed kernel values, lazy-initialized via `OnceLock`
- `SoftClampAccum`: (sp, sn, wp, wn) accumulator for deringing

## Kernel Correctness Analysis

### Lanczos Kernel (`mod.rs:55-65`)

The Lanczos kernel is defined as:
```
L(x) = sinc(pi*x) * sinc(pi*x/a)  for |x| < a
L(0) = 1
L(x) = 0                            for |x| >= a
```
where `sinc(t) = sin(t)/t`. The implementation matches this exactly:
- `x.abs() < 1e-6` returns 1.0 (correct: L(0) = 1 by L'Hopital)
- `x.abs() >= a` returns 0.0
- Otherwise: `(pi_x.sin() / pi_x) * (pi_x_a.sin() / pi_x_a)`

This matches the standard mathematical definition. **Correct.**

**Separable vs radial 2D application:** The code uses the separable formulation
`L(x,y) = L(x)*L(y)`, which is the standard and correct 2D Lanczos formulation. Research
confirms that the separable product of 1D sinc filters is the mathematically correct 2D
reconstruction filter (not an approximation of the radial version). The radial version
would require jinc functions (Bessel J1) and would be isotropic but much more expensive.
For astrophotography registration, the separable form is correct and matches what all major
tools use (PixInsight, SWarp, OpenCV, Siril).

**Normalization behavior:** The raw Lanczos kernel's integral is not exactly 1.0. For
Lanczos-3, the continuous integral is ~0.997 (per NASA SkyView analysis). Without
per-sample normalization, resampled images would be ~0.3% fainter on average (squared
for 2D = ~0.6%). Our implementation normalizes weights to sum to 1.0, avoiding this.
SkyView had this bug and had to fix it. **Our normalization is correct and necessary.**

### Lanczos LUT (`mod.rs:50-107`)

**Resolution:** 4096 samples per unit interval. For Lanczos3, total entries = 3 * 4096 + 1
= 12289, occupying ~48 KB (fits L1 cache on most CPUs).

**Precision analysis:** With 4096 samples/unit, nearest-index lookup gives max quantization
error ~1/(2*4096) = 0.000122 in the x-coordinate. The Lanczos3 kernel's maximum derivative
is approximately |dL/dx| ~ 3.5 near x=0.6, so worst-case LUT error is ~0.000122 * 3.5 =
~0.00043. This is adequate for f32 image data (~7 decimal digits), and tests confirm
LUT-vs-direct error < 0.001 (`tests.rs:93`).

**Comparison with industry:**
- PixInsight uses LUT accuracy of 1/2^16 (~65536 samples/unit) for integer images.
  For f32 astronomical data, our 4096 is sufficient.
- The lookup uses nearest-index rounding (`+ 0.5`) rather than linear interpolation
  between adjacent entries. Linear interpolation would reduce the error by ~100x at the
  cost of one extra multiply-add per lookup (12 lookups per pixel for Lanczos3).
- Intel IPP uses function evaluation with AVX, not LUT. Their separable approach
  amortizes the cost differently (only 12 evals per pixel instead of 36).

**Safety:** The `+ 0.5` rounding could produce an index at `num_entries - 1` when `abs_x`
is close to `a`. The guard at `abs_x >= self.a as f32` prevents this for exact boundary
values, and the `+1` in `num_entries` provides safety margin. Safe.

**`lookup_positive()` optimization:** Skips `abs()` and bounds check for known non-negative
distances within [0, a]. Used in the optimized Lanczos inner loop where fractional parts
are computed such that all distances are known-positive. Saves 2 branches per lookup.

### Kernel Weight Computation (`warp/mod.rs:259-272`)

For Lanczos3 with `fx` = fractional x-offset (0 <= fx < 1), pixel window starts at
`kx0 = x0 - 2`, covering 6 pixels: `[x0-2, x0-1, x0, x0+1, x0+2, x0+3]`.

Distances from sample point `x0 + fx` to each pixel:
- `wx[0] = L(fx + 2)`: pixel at x0-2, distance = fx + 2 (in [2, 3))
- `wx[1] = L(fx + 1)`: pixel at x0-1, distance = fx + 1 (in [1, 2))
- `wx[2] = L(fx)`:     pixel at x0,   distance = fx     (in [0, 1))
- `wx[3] = L(1 - fx)`: pixel at x0+1, distance = 1 - fx (in (0, 1])
- `wx[4] = L(2 - fx)`: pixel at x0+2, distance = 2 - fx (in (1, 2])
- `wx[5] = L(3 - fx)`: pixel at x0+3, distance = 3 - fx (in (2, 3])

All absolute distances are within [0, 3] = Lanczos3 support. The optimized path uses
`lookup_positive()` with explicit distance formulas that are always non-negative,
avoiding the abs() call in the general `lookup()`. **Correct.**

### Kernel Normalization

**Generic path** (`mod.rs`): Normalizes x and y weights independently:
`inv_wx = 1/wx_sum`, `inv_wy = 1/wy_sum`, applied per-sample as `wxi * inv_wx * wyj`.

**Optimized path** (`warp/mod.rs`): Uses combined normalization:
`inv_total = 1 / (wx_sum * wy_sum)`, applied as `sp * inv_total`.

These are mathematically equivalent: `sum(v * wxi * wyj) / (wx_sum * wy_sum)`.
**Identical.** Correct.

**Deringing path normalization:** The deringing code does NOT pre-normalize weights before
accumulating sp/sn/wp/wn. The `soft_clamp()` function computes ratios `r = sn/sp` and
`(sp - sn*c) / (wp - wn*c)`. Scaling all accumulators by a constant `k` cancels out:
`r' = k*sn/(k*sp) = r`. **Equivalent.** Correct.

### Bicubic Catmull-Rom (`mod.rs:125-135`)

Uses `a = -0.5` (Catmull-Rom), the standard interpolating cubic:
```
|x| <= 1: (a+2)|x|^3 - (a+3)|x|^2 + 1
1 < |x| < 2: a|x|^3 - 5a|x|^2 + 8a|x| - 4a
|x| >= 2: 0
```

Verified properties:
- `K(0) = 1`, `K(1) = 0`, `K(2) = 0`. All correct.
- Continuity at x=1: both pieces give 0. Correct.
- Weights sum to 1 (partition of unity) by construction for the Catmull-Rom family.

**Catmull-Rom in Mitchell-Netravali space:** B=0, C=0.5. Sharper than Mitchell-Netravali
(B=1/3, C=1/3) but may produce ringing on high-contrast edges. Mitchell-Netravali is
better for general-purpose resizing. For astrophotography registration, Catmull-Rom is
the correct choice because sharpness preservation matters and Lanczos is available as a
higher-quality alternative. This matches PixInsight's analysis showing Lanczos-3 preserves
96.9% of noise pixels after small rotation vs 65.1% for Mitchell-Netravali. **Correct choice.**

### Bilinear (`mod.rs:167-182`)

Standard bilinear interpolation using floor-based integer coordinates and linear blending:
```
top = p00 + fx * (p10 - p00)
bottom = p01 + fx * (p11 - p01)
result = top + fy * (bottom - top)
```
This is the standard formulation. **Correct.**

### Nearest Neighbor (`mod.rs:155-164`)

Uses `x.round()` which rounds 0.5 away from zero (Rust f32::round behavior).
Standard. **Correct.**

## Deringing Analysis vs Industry Standards

### PixInsight Soft Clamping (Reference Implementation)

PixInsight PCL's `LanczosInterpolation.h` uses a soft clamping algorithm (BSD license).
During accumulation each `weighted_value = pixel * weight` is split into:
- Positive contributions: `sp += val`, `wp += weight` (when `s >= 0`)
- Negative contributions: `sn += |val|`, `wn += |weight|` (when `s < 0`)

After accumulation:
```
r = sn / sp
if r >= 1.0: return sp / wp            // hard clamp
if r > threshold:
    fade = (r - threshold) / (1 - threshold)
    c = 1 - fade^2                      // quadratic fade
    return (sp - sn * c) / (wp - wn * c)
return (sp - sn) / (wp - wn)            // normal
```

Default threshold: 0.3. Lower = more aggressive deringing.

### Our Implementation (`warp/mod.rs:148-162`)

**Matches PixInsight exactly:**
- Accumulation logic: identical (split on `s < 0` vs `s >= 0`)
- Ratio computation: identical (`r = sn / sp`)
- Hard clamp at r >= 1: identical (`sp / wp`)
- Quadratic fade: identical (`c = 1 - fade^2`, `fade = (r-th)/(1-th)`)
- Final value: identical (`(sp - sn*c) / (wp - wn*c)`)
- Extra guard: our code handles `sp == 0` (returns 0.0). Safer.
- `th_inv` pre-computation avoids a division per pixel.

### Comparison with Other Tools

- **SWarp:** No deringing. Uses Lanczos4 by default, relies on wider kernel having less
  ringing. Flux conservation via Jacobian determinant multiplication (negligible for
  registration at ~1:1 scale).
- **Siril:** Uses Lanczos-4 by default with optional clamping.
- **OpenCV:** INTER_LANCZOS4 (8x8 kernel), no built-in deringing.
- **GNU Astronomy Utilities:** Uses "pixel mixing" (area resampling) instead of
  parametric interpolation, treating pixels as areas rather than points. Non-parametric,
  flux-conserving by construction. Appropriate for photometry but slower and smoother
  than Lanczos. Not applicable to our registration use case where sharpness matters.
- **LSST Pipeline:** Uses configurable warp kernel (Lanczos) with WCS grid interpolation
  for performance (2x speedup from not computing WCS per pixel).

### SIMD Deringing Path (`warp/sse.rs`)

Uses branchless SSE mask splitting:
```
pos_mask = cmpge(s, zero);  neg_mask = cmplt(s, zero);
sp += and(pos_mask, s);     sn -= and(neg_mask, s);
wp += and(pos_mask, w);     wn -= and(neg_mask, w);
```

Correct: `_mm_and_ps(mask, value)` selects values where mask bits are set. The subtraction
for `sn`/`wn` accumulates absolute values (since `s < 0` means `and(neg, s)` is negative,
and `sn -= negative` adds the absolute value). **Correct.**

## SIMD Implementation Analysis

### Generic Lanczos FMA Kernel (`warp/sse.rs`)

`lanczos_kernel_fma<A, SIZE, DERINGING>` -- const-generic over all Lanczos sizes:
- **Lanczos2 (SIZE=4):** Single `__m128` (4 weights), one 128-bit load per row
- **Lanczos3 (SIZE=6):** Two `__m128` (lo: 4 weights, hi: 2 weights + 2 zeros). Reads 8
  floats per row (2 zero-padded)
- **Lanczos4 (SIZE=8):** Two `__m128` (lo: 4, hi: 4 via `_mm_loadu_ps`). Reads 8 floats
  per row

The `SIZE > 4` branches are const-evaluable -- LLVM eliminates dead paths for each
monomorphization. Lanczos2 skips all hi-register code entirely.

Uses `_mm_fmadd_ps` in the non-deringing path: `sx = mul(src, wx); acc = fmadd(sx, wy, acc)`.

The deringing path uses `_mm_mul_ps` + `_mm_add_ps` because it needs both `w` and `s`
as separate values for mask-based positive/negative contribution tracking.

**Horizontal sum** (`hsum_ps`): Standard SSE reduction using
`movehdup + add + movehl + add_ss`. 3 instructions.

### AVX2 Bilinear (`warp/sse.rs:21-153`)

Processes 8 pixels per iteration:
1. SIMD coordinate computation: handles projective divide correctly
2. Scalar pixel sampling: 32 `sample_pixel` calls per chunk (bottleneck)
3. SIMD bilinear blending

**Bottleneck:** Step 2 -- scalar sampling with bounds checks. For interior pixels
(the vast majority), bounds checks always pass. A fast path that checks the whole 8-pixel
chunk bounds once and uses unchecked loads would eliminate 32 branches per chunk.

**Transform precision:** f64 matrix is converted to f32 once per row. For 4096-pixel width,
worst-case coordinate error from f32 truncation is ~4096 * 1.2e-7 = ~0.0005 pixels.
Acceptable for bilinear (1-pixel neighborhood) but would be marginal for Lanczos (6-pixel
neighborhood). The Lanczos path uses f64 incremental stepping, avoiding this issue.

### SSE4.1 Bilinear (`warp/sse.rs:163-278`)

Same algorithm as AVX2 but 4 pixels at a time. Uses `_mm_floor_ps` (SSE4.1 requirement).

### `fast_floor_i32` (`warp/mod.rs:28-31`)

```rust
fn fast_floor_i32(x: f32) -> i32 {
    let i = x as i32;
    i - (x < i as f32) as i32
}
```

Avoids libc `floorf` function call by truncating then correcting for negative values.
The standard Rust `as i32` truncates toward zero, so for negative values the correction
subtracts 1. Example: `x = -0.5`, `i = 0`, `x < 0.0` is true, result = -1. **Correct.**

Industry note: SIMD paths can use `_mm256_floor_ps`/`_mm_floor_ps` (SSE4.1) or
`_mm_cvtps_epi32` with rounding mode set, avoiding per-pixel function calls entirely.
The scalar `fast_floor_i32` is appropriate for the scalar fallback path.

## Performance Analysis

### Benchmark Results (from bench.rs and MEMORY.md)

Single-threaded 1024x1024, affine transform:
- Lanczos3 with deringing: 80.9ms (scalar) -> 33.4ms (SIMD) = **-59%**
- Lanczos3 without deringing: 36.6ms (scalar) -> 30.3ms (SIMD) = **-17%**
- Multi-threaded 4k Lanczos3: 95.7ms -> 45.7ms = **-52%**

The larger speedup with deringing (-59% vs -17%) is because deringing adds branchy
accumulation in scalar (if/else per sample) that the SIMD path handles branchlessly
with masked operations.

### Parallelization

Row-level rayon `par_chunks_mut`. Good for:
- Contiguous output writes (cache-friendly)
- Independent rows (no synchronization)
- Typical astro transforms (small rotation, ~1:1 scale) give cache-friendly input access

### LUT Cache Behavior

Lanczos3 LUT: 48 KB. Modern L1 data cache: 32-48 KB. The LUT is borderline for L1.
Each pixel does 12 LUT lookups at effectively random offsets within the LUT.
Reducing to 2048 samples/unit (24 KB) would guarantee L1 residency with error increase
from ~0.00043 to ~0.00086 (still adequate for f32).

### Memory Access Patterns

For a small-angle rotation (typical in astro registration), consecutive output pixels
map to nearly consecutive input pixels. The 6x6 kernel window for adjacent output
pixels overlaps heavily, so input data stays in L1/L2 cache. For large rotations
(>45 degrees), input access becomes stride-N which is cache-unfriendly. Tile-based
processing would help but adds complexity.

### Incremental Stepping Precision

The incremental stepping approach (`src_x += dx_step` per pixel) uses f64 arithmetic
for the accumulator. For a 4096-pixel row, worst-case accumulated error is approximately
4096 * machine_epsilon(f64) * |dx_step| ~ 4096 * 2.2e-16 * 1.0 ~ 9e-13 pixels, which
is completely negligible. No error correction (Kahan summation or periodic re-computation)
is needed. **Adequate.**

Note: the generic per-pixel loop in `warp_image()` also uses f64 incremental stepping
for Nearest/Bicubic. The bilinear AVX2/SSE paths use f32 coordinate computation (see
transform precision note above). All correct.

## Issues Found

### ~~1. SIMD Kernel Does Not Actually Use FMA~~ -- FIXED

No-deringing path now uses `_mm_fmadd_ps` with restructured computation:
`sx = mul(src, wx); acc = fmadd(sx, wy, acc)`. Saves 12 instructions per kernel call.
Measured: ~2.5% improvement on 1k single-threaded no-deringing benchmark.

### 2. Duplicate Bilinear Implementations (Low - Code Quality)

**Files:** `mod.rs:167-182` (`interpolate_bilinear`) and `warp/mod.rs:120-138`
(`bilinear_sample`)

These are functionally identical implementations of bilinear interpolation. Both take
`&Buffer2<f32>`. They should be consolidated into a single function. The `mod.rs` version
uses `x.floor()` while the `warp/mod.rs` version uses `fast_floor_i32()` -- the optimized
version should be kept.

### 3. No Anti-Aliasing Prefilter for Downscaling (Low - Context-Dependent)

When the transform involves scaling down (scale < 1), high-frequency content aliases into
the output. The current code does no prefiltering.

**For astrophotography:** Generally not an issue because:
- Registration maps images at ~1:1 scale
- Astronomical images are band-limited by the PSF (seeing/optics)
- Small scale differences (0.95x-1.05x) produce minimal aliasing

**What others do:**
- SWarp: no explicit prefilter, relies on Lanczos4 kernel being wider
- PixInsight: auto-selects Mitchell-Netravali for downsampling transforms
- OpenCV: `INTER_AREA` for downscaling, but `warpAffine` does NOT prefilter
- GNU Astronomy Utilities: uses area resampling (pixel mixing) which handles
  downscaling correctly by construction
- Siril: offers `area` interpolation option for downsampling

### 4. Bicubic/Bilinear Not Normalized at Boundaries (Low)

At image boundaries, `sample_pixel` returns `border_value` (default 0.0) for out-of-bounds
pixels. For bicubic/bilinear, the effective kernel weights don't sum to 1.0 near edges,
producing darkened pixels. This is expected for zero-padding borders and is standard
behavior (same as OpenCV BORDER_CONSTANT). Borders are cropped in astrophotography.

### 5. SIMD Bilinear border_value Hardcoded to 0.0 (Low)

The SIMD bilinear paths use hardcoded `0.0` for border_value. Non-zero border_value falls
back to scalar. Since 0.0 is the universal default for astrophotography, this has no
practical impact.

### 6. Bicubic Has No Deringing (Low)

The Bicubic (Catmull-Rom) kernel has negative lobes (a=-0.5 means K(1.5) = -0.0625) and
can produce ringing on high-contrast edges, similar to Lanczos. No deringing is
implemented for Bicubic. This is acceptable because:
- Bicubic's negative lobes are much smaller than Lanczos3's (~-0.06 vs ~-0.09)
- Users who care about ringing artifacts use Lanczos with deringing
- PixInsight does not offer deringing for bicubic either

## Potential Improvements (Prioritized)

### Worth Doing

1. ~~**Use actual FMA intrinsics in Lanczos SIMD kernel**~~ -- DONE. ~2.5% improvement.

2. ~~**Generic incremental stepping** for Lanczos2/Lanczos4/Bicubic~~ -- DONE.
   Lanczos2 -29.3%, Lanczos4 -45.4%, Lanczos3 -6.3%.

3. **Remove duplicate bilinear**: Consolidate `mod.rs:interpolate_bilinear` and
   `warp/mod.rs:bilinear_sample` into a single function. Use `fast_floor_i32`.

### Postponed (low impact for current use case)

- **Separable Lanczos** -- Does not apply to arbitrary warps with rotation. The separable
  sinc filter is technically correct (not an approximation) but only saves computation
  in axis-aligned resize (Intel IPP two-pass approach). For rotation warps, each pixel
  samples a different source position, so the two-pass approach provides no benefit.
- **SIMD bilinear interior fast path** -- Skip bounds checks for interior chunk. The
  scalar sampling inside the AVX2 bilinear path is the bottleneck (32 `sample_pixel`
  calls per 8-pixel chunk). A bounds check on the whole chunk could eliminate this, but
  was not benchmarked to confirm improvement. On modern CPUs (Skylake+), gather
  instructions have improved enough that `_mm256_i32gather_ps` might now be viable
  for bilinear (only 4 gathers needed, not the 6+ for Lanczos). Worth revisiting.
- **Linear interpolation in LUT** -- Current 4096 samples/unit is adequate for f32.
  Would reduce error ~100x at cost of 1 extra mul+add per lookup (12 per pixel).
- **Reduce LUT to 2048 samples/unit** -- Saves 24 KB, guarantees L1 residency. Error
  increases from ~0.00043 to ~0.00086, still adequate for f32.
- **Tile-based processing** -- Only helps extreme rotations (>45 deg), rare in practice.
- **Configurable border modes** -- Borders are cropped in astrophotography.
- **SIMD bilinear border_value** -- Currently hardcoded to 0.0. Low impact.
- **Flux conservation (Jacobian)** -- SWarp multiplies by Jacobian determinant. Negligible
  for registration (~1:1 scale). Would matter for mosaic with different plate scales.
- **Pixel mixing / area resampling** -- GNU Astronomy Utilities approach. Better flux
  conservation and no ringing by construction, but slower and introduces smoothing.
  Appropriate for photometry-critical applications, not for registration where sharpness
  and speed matter.
- **Isotropic Lanczos (jinc-based)** -- The separable `L(x)*L(y)` formulation is
  anisotropic. A truly isotropic kernel would use `jinc(r)*jinc(r/a)` where
  `r = sqrt(x^2+y^2)` and `jinc(t) = J1(pi*t)/(pi*t)`. This eliminates
  orientation-dependent artifacts but requires Bessel function evaluation and is
  much more expensive. No major astronomical tool uses this. The separable version
  is the universal standard and produces no visible artifacts at typical registration
  angles. Not worth implementing.

### Tried and Rejected

- **AVX2 gather for LUT lookups:** `_mm256_i32gather_ps` for 6 weights at once. ~2% slower
  than scalar lookups due to high gather latency (~12 cycles) vs L1-cached scalar loads
  (~4 cycles). The 48KB LUT fits in L1 cache, making scalar lookups fast enough.
- **Per-pixel separable factorization:** Restructuring inner loop from `v*wx*wy` to
  row_sums then vertical combine. No improvement -- LLVM already optimizes the original
  pattern.
- **aarch64 NEON bilinear:** Benchmarked and found slower than scalar. The NEON gather
  overhead (no hardware gather, must do scalar loads + `vld1q_lane_f32`) negated savings
  for a 2x2 kernel.
- **aarch64 NEON Lanczos:** Two approaches tried (per-row horizontal reduction, vertical
  accumulation with `vfmaq_f32`). Neither improved over scalar. Root cause: Lanczos is
  memory-bound, the 6x6 kernel accesses 6 non-contiguous rows at arbitrary positions.
  Cache misses dominate, and LLVM's auto-vectorization of the scalar fast-path (using
  `get_unchecked`) already generates efficient code.

## Industry Standards Comparison

### What We Do Right (Matches Best Practice)

| Feature | Our Implementation | Industry Standard |
|---------|-------------------|-------------------|
| Default kernel | Lanczos-3 | PixInsight: Lanczos-3, SWarp: Lanczos-3/4 |
| Deringing | PixInsight-style soft clamp, th=0.3 | PixInsight PCL exact match |
| LUT kernel eval | 4096 samples/unit, nearest lookup | PixInsight: 2^16 for int, adequate for f32 |
| Weight normalization | Per-pixel sum-to-one normalization | Required (SkyView had 0.3% error without) |
| 2D application | Separable L(x)*L(y) | Universal standard (not an approximation) |
| Inverse mapping | Output -> transform -> sample input | OpenCV, SWarp, all standard tools |
| Incremental stepping | f64 for affine transforms | Standard optimization for scan conversion |
| Multi-threading | Row-level rayon parallelism | SWarp: row parallelism, OpenCV: similar |
| Kernel sizes | L2 (4x4), L3 (6x6), L4 (8x8) | PixInsight: L3-L5, OpenCV: L4 only |
| Bicubic variant | Catmull-Rom (a=-0.5) | PixInsight offers CR + Mitchell + B-spline |
| SIP distortion | Falls back to per-pixel transform | LSST: grid interpolation for performance |

### What We Don't Do (Industry Features Not Implemented)

| Feature | Who Does It | Why We Skip It |
|---------|-------------|----------------|
| Flux conservation (Jacobian) | SWarp | Negligible at ~1:1 registration scale |
| Anti-aliasing prefilter | PixInsight auto-select | Registration is ~1:1, PSF band-limits |
| Area resampling (pixel mixing) | GNU Astro Utils, Siril | Slower, smooths; registration needs sharpness |
| Mitchell-Netravali bicubic | PixInsight | Catmull-Rom is sharper, Lanczos is better |
| Lanczos-5 | PixInsight | Diminishing returns, L4 already has 8x8 kernel |
| Isotropic jinc kernel | Nobody (research only) | Expensive, no visible benefit |
| B-spline interpolation | PixInsight | Smoothing filter, not suitable for registration |
| WCS grid interpolation | LSST pipeline | We use SIP polynomial directly, adequate |
| Weight mask warping | LSST pipeline | Not needed for single-channel astro data |

### What We Do That Others Don't

| Feature | Notes |
|---------|-------|
| Const-generic Lanczos (2/3/4 unified) | Most tools have separate per-size code |
| SSE FMA kernel generic over all sizes | Unusual; most have L3-only or L4-only SIMD |
| `lookup_positive()` fast LUT path | Skips abs+bounds for known-positive distances |
| f64 incremental stepping | Some tools use f32 (OpenCV) with more drift |
| Deringing for all Lanczos sizes | PixInsight applies to L3 only by default |

## Research Sources

- [Lanczos Resampling (Wikipedia)](https://en.wikipedia.org/wiki/Lanczos_resampling)
- [Lanczos Interpolation Explained (Mazzo)](https://mazzo.li/posts/lanczos.html)
- [PixInsight Interpolation Algorithms](https://pixinsight.com/doc/docs/InterpolationAlgorithms/InterpolationAlgorithms.html)
- [PixInsight PCL Source (GitHub)](https://github.com/PixInsight/PCL/blob/master/include/pcl/Interpolation.h)
- [PixInsight Forum: New Lanczos Algorithm](https://pixinsight.com/forum/index.php?threads/new-lanczos-pixel-interpolation-algorithm-imageregistration-geometry-modules.3734/)
- [Intel IPP AVX Lanczos](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html)
- [AVIR Lanczos Resizer (GitHub)](https://github.com/avaneev/avir)
- [SWarp User Guide](https://star.herts.ac.uk/~pwl/Lucas/rho_oph/swarp.pdf)
- [SWarp Source (GitHub)](https://github.com/astromatic/swarp)
- [Mitchell-Netravali Filters (Wikipedia)](https://en.wikipedia.org/wiki/Mitchell%E2%80%93Netravali_filters)
- [Mitchell & Netravali 1988 (PDF)](https://www.cs.utexas.edu/~fussell/courses/cs384g-fall2013/lectures/mitchell/Mitchell.pdf)
- [OpenCV Geometric Transforms](https://docs.opencv.org/4.x/da/d54/group__imgproc__transform.html)
- [Siril Registration Docs](https://siril.readthedocs.io/en/latest/preprocessing/registration.html)
- [SkyView Lanczos Normalization Error](https://skyview.gsfc.nasa.gov/blog/index.php/2016/02/04/lanczos-normalization-error/)
- [GNU Astronomy Utilities: Resampling](https://www.gnu.org/software/gnuastro/manual/html_node/Resampling.html)
- [LSST Warper API](https://pipelines.lsst.io/py-api/lsst.afw.math.Warper.html)
- [Correct 2D Lanczos (pixelflinger)](https://github.com/pixelflinger/lanczos-2d)
- [KLERP: Fast AVX2 Bilinear](https://github.com/komrad36/KLERP)
- [Efficient Lanczos on ARM (PDF)](https://www.scitepress.org/papers/2018/65470/65470.pdf)
- [Bart Wronski: Bilinear Downsampling](https://bartwronski.com/2021/02/15/bilinear-down-upsampling-pixel-grids-and-that-half-pixel-offset/)
- [OpenCV warpAffine SIMD PR #26505](https://github.com/opencv/opencv/pull/26505)
