# Interpolation / Warp Module -- Research Analysis

## Architecture Overview

This module provides image interpolation and warping for astronomical image registration.
It supports six interpolation methods: Nearest, Bilinear, Bicubic (Catmull-Rom a=-0.5),
Lanczos2, Lanczos3 (default), and Lanczos4. Warping is performed via `warp_image()` which
parallelizes row processing with rayon. Bilinear and Lanczos3 have dedicated optimized
row-warping paths; all others go through a generic per-pixel loop.

**Files:**
- `mod.rs` -- Kernel functions, LUT, per-pixel interpolation, `warp_image` dispatcher
- `warp/mod.rs` -- Row-level warping: bilinear scalar, Lanczos3 optimized, SIMD dispatch
- `warp/sse.rs` -- AVX2/SSE4.1 SIMD bilinear + SSE FMA Lanczos3 kernel
- `tests.rs` -- Unit and quality tests for kernels, interpolation, and warping
- `bench.rs` -- Benchmarks for 1k/2k/4k Lanczos3 warps, bilinear, LUT lookup

**Data flow:**
1. `warp_image()` (`mod.rs:288`) dispatches per-row via rayon `par_chunks_mut`
2. For Bilinear: `warp_row_bilinear()` -> SIMD (AVX2/SSE4.1) or scalar
3. For Lanczos3: `warp_row_lanczos3()` -> const-generic `DERINGING` dispatch ->
   SIMD FMA kernel (interior) or scalar fast path (interior) or slow path (border)
4. For other methods: generic per-pixel `interpolate()` loop
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
where `sinc(t) = sin(t)/t`. The implementation at `mod.rs:55-65` matches this exactly:
- `x.abs() < 1e-6` returns 1.0 (correct: L(0) = 1 by L'Hopital)
- `x.abs() >= a` returns 0.0
- Otherwise: `(pi_x.sin() / pi_x) * (pi_x_a.sin() / pi_x_a)`

This matches the standard mathematical definition from Wikipedia and the Lanczos
resampling literature. **Correct.**

### Lanczos LUT (`mod.rs:50-95`)

**Resolution:** 4096 samples per unit interval. For Lanczos3, total entries = 3 * 4096 + 1
= 12289, occupying ~48 KB (fits L1 cache on most CPUs).

**Precision analysis:** With 4096 samples/unit, nearest-index lookup gives max quantization
error ~1/(2*4096) = 0.000122 in the x-coordinate. The Lanczos3 kernel's maximum derivative
is approximately |dL/dx| ~ 3.5 near x=0.6, so worst-case LUT error is ~0.000122 * 3.5 =
~0.00043. This is adequate for f32 image data (~7 decimal digits), and tests confirm
LUT-vs-direct error < 0.001 (`tests.rs:43`).

**Comparison with industry:**
- PixInsight uses LUT accuracy of 1/2^16 (~65536 samples/unit) for integer images.
  For f32 astronomical data, our 4096 is sufficient.
- The lookup uses nearest-index rounding (`+ 0.5` at `mod.rs:92`) rather than linear
  interpolation between adjacent entries. Linear interpolation would reduce the error
  by ~100x at the cost of one extra multiply-add per lookup (12 lookups per pixel).
- Intel IPP uses function evaluation with AVX, not LUT. Their separable approach
  amortizes the cost differently (only 12 evals per pixel instead of 36).

**Safety:** The `+ 0.5` rounding could produce an index at `num_entries - 1` when `abs_x`
is close to `a`. The guard at `mod.rs:89` (`abs_x >= self.a as f32`) prevents this for
exact boundary values, and the `+1` in `num_entries` at `mod.rs:75` provides safety margin.
Safe.

### Kernel Weight Computation (`warp/mod.rs:228-243`)

For Lanczos3 with `fx` = fractional x-offset (0 <= fx < 1), pixel window starts at
`kx0 = x0 - 2`, covering 6 pixels: `[x0-2, x0-1, x0, x0+1, x0+2, x0+3]`.

Distances from sample point `x0 + fx` to each pixel:
- `wx[0] = L(fx + 2)`: pixel at x0-2, distance = fx + 2 (in [2, 3))
- `wx[1] = L(fx + 1)`: pixel at x0-1, distance = fx + 1 (in [1, 2))
- `wx[2] = L(fx)`:     pixel at x0,   distance = fx     (in [0, 1))
- `wx[3] = L(fx - 1)`: pixel at x0+1, distance = 1 - fx (in (0, 1])
- `wx[4] = L(fx - 2)`: pixel at x0+2, distance = 2 - fx (in (1, 2])
- `wx[5] = L(fx - 3)`: pixel at x0+3, distance = 3 - fx (in (2, 3])

All absolute distances are within [0, 3] = Lanczos3 support. This matches the generic
`interpolate_lanczos_impl` which computes `fx - (i - A + 1)` for A=3: offsets are
`fx+2, fx+1, fx, fx-1, fx-2, fx-3`. **Correct.**

### Kernel Normalization

**Generic path** (`mod.rs:228-262`): Normalizes x and y weights independently:
`inv_wx = 1/wx_sum`, `inv_wy = 1/wy_sum`, applied per-sample as `wxi * inv_wx * wyj`.

**Optimized path** (`warp/mod.rs:245-252`): Uses combined normalization:
`inv_total = 1 / (wx_sum * wy_sum)`, applied as `sp * inv_total`.

These are mathematically equivalent. The generic path computes:
`sum_j(sum_i(v * (wxi/wx_sum) * (wyj/wy_sum)))` = `sum(v * wxi * wyj) / (wx_sum * wy_sum)`.
The optimized path computes `sp / (wx_sum * wy_sum)` where `sp = sum(v * wxi * wyj)`.
**Identical.** Correct.

**Deringing path normalization:** The deringing code does NOT pre-normalize weights before
accumulating sp/sn/wp/wn. The reference test in `warp/mod.rs:807-869` DOES pre-normalize.
However, `soft_clamp()` computes ratios `r = sn/sp` and `(sp - sn*c) / (wp - wn*c)`.
Scaling all accumulators by a constant `k` cancels out: `r' = k*sn/(k*sp) = r`, and
`(k*sp - k*sn*c) / (k*wp - k*wn*c) = (sp-sn*c)/(wp-wn*c)`. **Equivalent.** Correct.

### Bicubic Catmull-Rom (`mod.rs:112-123`)

Uses `a = -0.5` (Catmull-Rom), the standard interpolating cubic:
```
|x| <= 1: (a+2)|x|^3 - (a+3)|x|^2 + 1
1 < |x| < 2: a|x|^3 - 5a|x|^2 + 8a|x| - 4a
|x| >= 2: 0
```

Verified properties:
- `K(0) = 1`: `((-0.5+2)*0 - (-0.5+3)*0 + 1) = 1`. Correct.
- `K(1) = 0`: `(1.5*1 - 2.5*1 + 1) = 0`. Correct.
- `K(2) = 0`: `(-0.5*8 - 5*(-0.5)*4 + 8*(-0.5)*2 - 4*(-0.5)) = -4+10-8+2 = 0`. Correct.
- Continuity at x=1: both pieces give 0.
- Weights sum to 1 (partition of unity) for interior points by construction.

**Comparison:** Catmull-Rom (B=0, C=0.5 in Mitchell-Netravali space) is sharper than
Mitchell-Netravali (B=1/3, C=1/3) but may produce ringing on high-contrast edges.
Mitchell-Netravali is better for general-purpose resizing. For astrophotography registration,
Catmull-Rom is appropriate since sharpness is preferred and Lanczos is available for the
best quality. **Correct choice.**

### Bilinear (`mod.rs:155-170`)

Standard bilinear interpolation using floor-based integer coordinates and linear blending:
```
top = p00 + fx * (p10 - p00)
bottom = p01 + fx * (p11 - p01)
result = top + fy * (bottom - top)
```
This is the standard formulation. **Correct.**

### Nearest Neighbor (`mod.rs:143-152`)

Uses `x.round()` which rounds 0.5 to 1 (round half to even or half up depending on
platform, but consistent). Standard. **Correct.**

## Deringing Analysis vs Industry Standards

### PixInsight Soft Clamping (Reference Implementation)

PixInsight PCL's `LanczosInterpolation.h` uses a soft clamping algorithm. Based on the PCL
source code analysis (BSD license), during accumulation each `weighted_value = pixel * weight`
is split into:
- Positive contributions: `sp += val`, `wp += weight` (when `s >= 0`)
- Negative contributions: `sn += |val|`, `wn += |weight|` (when `s < 0`)

After accumulation:
```
r = sn / sp                           // ratio of negative to positive
if r >= 1.0: return sp / wp           // hard clamp (total undershoot)
if r > threshold:
    fade = (r - threshold) / (1 - threshold)
    c = 1 - fade^2                    // quadratic fade
    return (sp - sn * c) / (wp - wn * c)  // reduce negative contribution
return (sp - sn) / (wp - wn)          // normal (below threshold)
```

Default threshold: 0.3. Lower = more aggressive deringing.

### Our Implementation (`warp/mod.rs:137-151`)

```rust
fn soft_clamp(sp, sn, wp, wn, th, th_inv) -> f32 {
    if sp == 0.0 { return 0.0; }
    let r = sn / sp;
    if r >= 1.0 { return sp / wp; }
    if r > th {
        let fade = (r - th) * th_inv;  // th_inv = 1/(1-th)
        let c = 1.0 - fade * fade;
        return (sp - sn * c) / (wp - wn * c);
    }
    (sp - sn) / (wp - wn)
}
```

**Comparison with PixInsight:**
- Accumulation logic: identical (split on `s < 0` vs `s >= 0`)
- Ratio computation: identical (`r = sn / sp`)
- Hard clamp at r >= 1: identical (`sp / wp`)
- Quadratic fade: identical (`c = 1 - fade^2`, `fade = (r-th)/(1-th)`)
- Final value: identical (`(sp - sn*c) / (wp - wn*c)`)
- Extra guard: our code handles `sp == 0` (returns 0.0). PixInsight may assume
  sp > 0 for valid images. Our guard is safer.

**Verdict: Implementation matches PixInsight's algorithm exactly.** Default threshold 0.3
also matches. The `th_inv` pre-computation avoids a division per pixel.

### Comparison with Other Approaches

**Hard min/max clamping** (old approach, removed): Clamps output to [min, max] of the
kernel neighborhood. Simple but overly aggressive -- flattens legitimate interpolation
values near sharp edges. Our soft clamping is strictly superior.

**SWarp approach:** SWarp uses Lanczos4 by default for its resampling kernel. SWarp does
NOT implement deringing; it conserves flux via Jacobian determinant multiplication and
relies on the Lanczos4 kernel having less ringing than Lanczos3. For astronomical image
registration at ~1:1 scale, the ringing is minimal anyway.

**Siril approach:** Uses Lanczos-4 by default with optional clamping.

### SIMD Deringing Path (`warp/sse.rs:343-358`)

Uses branchless SSE mask splitting:
```
pos_mask = cmpge(s, zero);  neg_mask = cmplt(s, zero);
sp += and(pos_mask, s);     sn -= and(neg_mask, s);
wp += and(pos_mask, w);     wn -= and(neg_mask, w);
```

This is correct: `_mm_and_ps(mask, value)` selects the value where mask bits are set,
zeroes elsewhere. The `_mm_cmpge_ps` and `_mm_cmplt_ps` return all-ones or all-zeros
per lane, so `and` acts as a conditional select. The subtraction for `sn`/`wn`
accumulates the absolute values (since `s < 0` means `and(neg, s)` is negative, and
`sn -= negative` adds the absolute value). **Correct.**

## SIMD Implementation Analysis

### Lanczos3 FMA Kernel (`warp/sse.rs:306-381`)

**Naming issue:** The function is named `lanczos3_kernel_fma` and tagged with
`#[target_feature(enable = "avx2,fma")]`, but **does not use any FMA intrinsics**.
The accumulation uses `_mm_mul_ps` + `_mm_add_ps` (separate multiply and add), not
`_mm_fmadd_ps`. This means:
1. The function requires FMA support at runtime (dispatch checks `has_avx2_fma()`)
   but does not benefit from it.
2. Renaming to `lanczos3_kernel_sse` and changing the target feature to just `"sse4.1"`
   would make it available on more hardware without any loss.
3. Alternatively, replacing the mul+add pairs with actual FMA intrinsics would improve
   both accuracy (single rounding) and throughput (1 instruction instead of 2).

**Architecture:** Two `__m128` accumulators (lo/hi) process 4+4 = 8 floats per row.
6 rows x (2 loads + 2 weight muls + 2 src*weight muls) = 36 multiplies, 12 adds.
With deringing: additional 8 comparisons + 8 masked adds per row = 48 extra ops.

**Register pressure:** Without deringing: 2 accumulators + 2 wx + 1 zero + 2 src + 2
temps = ~9 XMM registers. With deringing: adds 8 accumulators (sp/sn/wp/wn lo/hi) = 17
registers. x86_64 has 16 XMM registers, so LLVM must spill 1 register. This is minimal
overhead.

**Horizontal sum** (`hsum_ps`): Standard SSE horizontal reduction using
`movehdup + add + movehl + add_ss`. 3 instructions. Correct and efficient.

### AVX2 Bilinear (`warp/sse.rs:21-153`)

Processes 8 pixels per iteration:
1. SIMD coordinate computation (lines 80-89): handles projective divide correctly
2. Scalar pixel sampling (lines 117-127): 32 `sample_pixel` calls per chunk
3. SIMD bilinear blending (lines 138-143)

**The bottleneck is step 2** -- scalar sampling with bounds checks. For interior pixels
(the vast majority), bounds checks always pass. A fast path that checks the whole 8-pixel
chunk bounds once and uses unchecked loads would eliminate 32 branches per chunk.

**Transform precision:** f64 matrix is converted to f32 once per row (lines 37-44).
For 4096-pixel width, worst-case coordinate error from f32 truncation is
~4096 * 1.2e-7 = ~0.0005 pixels. Acceptable for bilinear (1-pixel neighborhood) but
would be marginal for Lanczos (6-pixel neighborhood). The Lanczos3 path uses f64
incremental stepping, avoiding this issue.

### SSE4.1 Bilinear (`warp/sse.rs:163-278`)

Same algorithm as AVX2 but 4 pixels at a time. Uses `_mm_floor_ps` (SSE4.1 requirement).

## Performance Analysis

### Benchmark Results (from bench.rs and MEMORY.md)

Single-threaded 1024x1024, affine transform:
- Lanczos3 with deringing: 80.9ms (scalar) -> 33.4ms (SIMD) = **-59%**
- Lanczos3 without deringing: 36.6ms (scalar) -> 30.3ms (SIMD) = **-17%**
- Multi-threaded 4k Lanczos3: 95.7ms -> 45.7ms = **-52%**

The larger speedup with deringing (-59% vs -17%) is because deringing adds branchy
accumulation in scalar (if/else per sample) that the SIMD path handles branchlessly
with masked operations.

### Parallelization (`mod.rs:299-316`)

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

## Issues Found

### 1. SIMD Kernel Does Not Actually Use FMA (Medium - Correctness/Naming)

**File:** `warp/sse.rs:306-381`
**Lines:** 338-341

The function `lanczos3_kernel_fma` requires `#[target_feature(enable = "avx2,fma")]`
but uses `_mm_mul_ps` + `_mm_add_ps` instead of `_mm_fmadd_ps`. This:
- Restricts the function to CPUs with FMA support unnecessarily
- Misses the accuracy benefit of FMA (single rounding instead of double)
- Misses the throughput benefit (FMA is 1 uop on modern CPUs)
- The function name is misleading

**Fix:** Either:
(a) Replace `_mm_mul_ps(src, w) -> acc = _mm_add_ps(acc, s)` with
    `acc = _mm_fmadd_ps(src, w, acc)` to actually use FMA, or
(b) Change target feature to `"sse4.1"` and rename to drop "fma" if FMA is not needed.

Option (a) is preferred: FMA would improve both speed and accuracy, and the dispatch
already requires FMA.

### 2. Duplicate Bilinear Implementations (Low - Code Quality)

**Files:** `mod.rs:155-170` (`interpolate_bilinear`) and `warp/mod.rs:109-127`
(`bilinear_sample`)

These are functionally identical implementations of bilinear interpolation. The `mod.rs`
version takes `&Buffer2<f32>`, the `warp/mod.rs` version also takes `&Buffer2<f32>`.
They should be consolidated into a single function.

### 3. No Anti-Aliasing Prefilter for Downscaling (Low - Context-Dependent)

When the transform involves scaling down (scale < 1), high-frequency content aliases into
the output. The current code does no prefiltering.

**For astrophotography:** Generally not an issue because:
- Registration maps images at ~1:1 scale
- Astronomical images are band-limited by the PSF (seeing/optics)
- Small scale differences (0.95x-1.05x) produce minimal aliasing

**What others do:**
- SWarp: no explicit prefilter, relies on Lanczos4 kernel being wider
- PixInsight: no mention of prefilter in interpolation docs
- OpenCV: `INTER_AREA` for downscaling, but `warpAffine` does NOT prefilter
- Siril: offers `area` interpolation option for downsampling

### 4. Bicubic/Bilinear Not Normalized at Boundaries (Low)

At image boundaries, `sample_pixel` returns `border_value` (default 0.0) for out-of-bounds
pixels. For bicubic/bilinear, the effective kernel weights don't sum to 1.0 near edges,
producing darkened pixels. This is expected for zero-padding borders and is standard
behavior (same as OpenCV BORDER_CONSTANT). Borders are cropped in astrophotography.

### 5. SIMD Bilinear border_value Hardcoded to 0.0 (Low)

**File:** `warp/mod.rs:47-63`

The SIMD bilinear paths (`warp_row_bilinear_avx2`, `warp_row_bilinear_sse`) use hardcoded
`0.0` for border_value. Non-zero border_value falls back to scalar. Since 0.0 is the
universal default for astrophotography, this has no practical impact.

## Potential Improvements (Prioritized)

### Worth Doing

1. **Use actual FMA intrinsics in Lanczos3 SIMD kernel** (`warp/sse.rs:338-341`):
   Replace `_mm_mul_ps` + `_mm_add_ps` with `_mm_fmadd_ps`. Gives both accuracy
   (single rounding) and throughput improvements. The dispatch already requires FMA.
   Estimated improvement: ~5-10% for the SIMD kernel path.

2. **Generic incremental stepping** for Lanczos2/Lanczos4/Bicubic: Factor out stepping
   logic from the Lanczos3-specific row warper so all methods benefit from avoiding
   per-pixel matrix multiply. Estimated ~38% speedup for non-Lanczos3 methods.

3. **Remove duplicate bilinear**: Consolidate `mod.rs:interpolate_bilinear` and
   `warp/mod.rs:bilinear_sample` into a single function.

### Postponed (low impact)

- **Separable Lanczos3** -- does not apply to arbitrary warps with rotation. Only
  useful for axis-aligned resize (Intel IPP two-pass approach).
- **SIMD bilinear interior fast path** -- skip bounds checks for interior chunk. Marginal.
- **Linear interpolation in LUT** -- current 4096 samples/unit is adequate for f32.
  Would reduce error ~100x at cost of 1 extra mul+add per lookup (12 per pixel).
- **Reduce LUT to 2048 samples/unit** -- saves 24 KB, guarantees L1 residency. Error
  increases from ~0.00043 to ~0.00086, still adequate.
- **Tile-based processing** -- only helps extreme rotations (>45 deg), rare in practice.
- **Configurable border modes** -- borders are cropped in astrophotography.
- **SIMD bilinear border_value** -- currently hardcoded to 0.0. Low impact.
- **Flux conservation (Jacobian)** -- SWarp multiplies by Jacobian determinant. Negligible
  for registration (~1:1 scale). Would matter for mosaic with different plate scales.

### Tried and Rejected

- **AVX2 gather for LUT lookups:** `_mm256_i32gather_ps` for 6 weights at once. ~2% slower
  than scalar lookups due to high gather latency (~12 cycles) vs L1-cached scalar loads
  (~4 cycles). The 48KB LUT fits in L1 cache, making scalar lookups fast enough.
- **Per-pixel separable factorization:** Restructuring inner loop from `v*wx*wy` to
  row_sums then vertical combine. No improvement -- LLVM already optimizes the original
  pattern.

## Research Sources

- [Lanczos Resampling (Wikipedia)](https://en.wikipedia.org/wiki/Lanczos_resampling) -- Mathematical definition: L(x) = sinc(pi*x) * sinc(pi*x/a) for |x| < a. Our implementation matches exactly.
- [Lanczos Interpolation Explained (Mazzo)](https://mazzo.li/posts/lanczos.html) -- Clear explanation of windowed sinc, ringing, normalization.
- [PixInsight Interpolation Algorithms](https://pixinsight.com/doc/docs/InterpolationAlgorithms/InterpolationAlgorithms.html) -- Recommends Lanczos-3 with clamping threshold 0.3 for registration. LUT accuracy 1/2^16 for integer images.
- [PixInsight PCL Source (GitHub)](https://github.com/PixInsight/PCL/blob/master/include/pcl/Interpolation.h) -- Reference implementation of soft clamping algorithm. Our implementation matches.
- [PixInsight Forum: New Lanczos Algorithm](https://pixinsight.com/forum/index.php?threads/new-lanczos-pixel-interpolation-algorithm-imageregistration-geometry-modules.3734/) -- Discussion of clamping threshold selection, default 0.3.
- [Intel IPP AVX Lanczos](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html) -- Two-pass separable Lanczos3 with AVX SIMD. 42 mults + 35 adds per pixel. 1.5x SSE->AVX gain.
- [AVIR Lanczos Resizer (GitHub)](https://github.com/avaneev/avir) -- High-quality SIMD Lanczos resizer with AVX2/SSE2/NEON. LANCIR variant is 3x faster.
- [SWarp User Guide](https://star.herts.ac.uk/~pwl/Lucas/rho_oph/swarp.pdf) -- Recommends Lanczos3/4 for astronomical resampling. Flux conservation via Jacobian. No deringing.
- [SWarp Source (GitHub fork)](https://github.com/corbettht/swarp) -- Fork with additional interpolation kernels.
- [Mitchell-Netravali Filters (Wikipedia)](https://en.wikipedia.org/wiki/Mitchell%E2%80%93Netravali_filters) -- B/C parameter space. Catmull-Rom is (B=0, C=0.5), Mitchell is (B=1/3, C=1/3).
- [Mitchell & Netravali 1988 (PDF)](https://www.cs.utexas.edu/~fussell/courses/cs384g-fall2013/lectures/mitchell/Mitchell.pdf) -- Original paper. Recommended (1/3, 1/3) as best compromise.
- [OpenCV Geometric Transforms](https://docs.opencv.org/4.x/da/d54/group__imgproc__transform.html) -- Uses inverse mapping for warp. BORDER_CONSTANT with configurable value.
- [Siril Registration Docs](https://siril.readthedocs.io/en/latest/preprocessing/registration.html) -- Uses Lanczos-4 by default with clamping option.
- [Bart Wronski: Bilinear Downsampling](https://bartwronski.com/2021/02/15/bilinear-down-upsampling-pixel-grids-and-that-half-pixel-offset/) -- Anti-aliasing requirements for downscaling.
- [AstroPixelProcessor: Interpolation Artifacts](https://www.astropixelprocessor.com/community/tutorials-workflows/interpolation-artifacts/) -- Practical examples of Lanczos ringing around stars.
- [Efficient Lanczos on ARM (PDF)](https://www.scitepress.org/papers/2018/65470/65470.pdf) -- SIMD Lanczos techniques for ARM NEON, applicable patterns for SSE/AVX.
