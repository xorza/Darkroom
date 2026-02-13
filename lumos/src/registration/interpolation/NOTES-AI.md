# Interpolation / Warp Module -- Research Analysis

## Overview

This module provides image interpolation and warping for astronomical image registration.
It supports six interpolation methods: Nearest, Bilinear, Bicubic (Catmull-Rom a=-0.5),
Lanczos2, Lanczos3 (default), and Lanczos4. Warping is performed via `warp_image()` which
parallelizes row processing with rayon. Bilinear and Lanczos3 have dedicated optimized
row-warping paths; all others go through a generic per-pixel loop.

**Files:**
- `mod.rs` -- Kernel functions, LUT, per-pixel interpolation, `warp_image` dispatcher
- `warp/mod.rs` -- Row-level warping: bilinear scalar, Lanczos3 optimized, SIMD dispatch
- `warp/sse.rs` -- AVX2 and SSE4.1 SIMD bilinear implementations
- `tests.rs` -- Unit and quality tests for kernels, interpolation, and warping
- `bench.rs` -- Benchmarks for 1k/2k/4k Lanczos3 warps, bilinear, LUT lookup

## Kernel Correctness Analysis

### Lanczos LUT (`mod.rs` lines 23-83)

**Resolution:** 4096 samples per unit interval. For Lanczos3, total entries = 3 * 4096 + 1
= 12289, occupying ~48 KB (fits L1 cache).

**Precision analysis:** With 4096 samples/unit, nearest-index lookup gives max quantization
error ~1/(2*4096) = 0.000122 in the x-coordinate. The Lanczos3 kernel's maximum derivative
is approximately |dL/dx| ~ 3.5 near x=0.6, so worst-case LUT error is ~0.000122 * 3.5 =
~0.00043. This is adequate for f32 image data (which has ~7 decimal digits of precision),
and tests confirm LUT-vs-direct error < 0.001 (`tests.rs` line 39).

**Observation:** The lookup uses nearest-index rounding (`+ 0.5` at line 66) rather than
linear interpolation between adjacent entries. Linear interpolation would reduce the error
by ~100x at the cost of one extra multiply-add. PixInsight uses LUT accuracy of 1/2^16
(~65536 samples/unit) for integer images. For f32 astronomical data, the current 4096 is
sufficient but could be improved cheaply if needed.

**Potential issue:** The `+ 0.5` in the LUT lookup (`mod.rs` line 66) implements rounding.
This is correct for nearest-index lookup. However, if `abs_x` is very close to `a` (the
support boundary), the rounded index could point past the last valid entry. The guard at
line 63 (`abs_x >= self.a as f32`) prevents this for exact boundary values, and the `+1`
in `num_entries` at line 49 provides one extra entry as safety margin. This is safe.

### Kernel Normalization (`mod.rs` lines 194-214, `warp/mod.rs` lines 181-202)

Normalization is applied correctly in both the generic `interpolate_lanczos_impl` and the
optimized `warp_row_lanczos3`. The x and y weight sums are computed independently, and
the threshold `1e-10` protects against division by zero for edge cases (e.g., pixel fully
outside the image).

**Config wiring:** `border_value` is wired through via `WarpParams` struct (method +
border_value). `normalize_kernel` was removed (normalization is always applied). Deringing
is a field on the Lanczos variants of `InterpolationMethod` (e.g.
`Lanczos3 { deringing: true }`), using min/max clamping of source pixels in the kernel
window.

### Bicubic Catmull-Rom (`mod.rs` lines 85-97)

Uses `a = -0.5` (Catmull-Rom), which is the standard interpolating cubic that passes
through data points exactly. This is correct for registration where preserving pixel values
at grid points matters.

**For astrophotography:** Catmull-Rom is a reasonable choice. PixInsight recommends
Lanczos-3 as best for registration due to superior detail preservation with minimal
aliasing. Mitchell-Netravali (B=1/3, C=1/3) would be better for downsampling but worse
for registration. The current Catmull-Rom implementation is sound for the bicubic option.

**Not normalized:** Unlike Lanczos, the bicubic kernel is not explicitly normalized in
`interpolate_bicubic` (`mod.rs` lines 138-167). Catmull-Rom weights sum to 1.0 by
construction for interior points, so this is correct. At boundaries where `sample_pixel`
returns 0.0 for out-of-bounds pixels, the effective sum is reduced, causing darkening.
This matches the bilinear boundary behavior and is the expected behavior for
zero-padding borders.

### Nearest Neighbor (`mod.rs` lines 110-118)

Uses `x.round()`, which rounds half-values up (0.5 -> 1). This is standard. The
`sample_pixel` call handles out-of-bounds correctly, returning 0.0.

### `sample_pixel` Border Handling (`mod.rs`)

Returns `border_value` for out-of-bounds coordinates (default 0.0 = zero-padding). This
means bilinear/bicubic/Lanczos near image edges will use the configured border value.
`Config.border_value` is wired through via `WarpParams`.

**Industry comparison:** OpenCV offers `BORDER_REPLICATE` (clamp-to-edge),
`BORDER_REFLECT`, `BORDER_CONSTANT` (configurable value), etc. PixInsight uses clamped
borders. The current constant-value padding is acceptable for astrophotography where
borders are cropped.

## Warp Implementation Analysis

### Incremental Stepping (`warp/mod.rs` lines 63-84, 132-243)

For affine transforms (Translation, Euclidean, Similarity, Affine), the source coordinate
increments linearly as `x` advances by 1:
- `dx_step = m[0]` (line 69) -- correct: d(src_x)/dx = m[0] for affine
- `dy_step = m[3]` (line 70) -- correct: d(src_y)/dx = m[3] for affine

The `is_linear()` check (`transform.rs` lines 361-363) correctly excludes Homography and
SIP transforms, both of which have nonlinear coordinate mappings where incremental stepping
would be wrong.

**Correctness verified:** The initial point `src0 = wt.apply(DVec2::new(0.0, y))` is
computed with full precision (f64), and stepping accumulates in f64. For a 4096-wide image,
worst-case accumulated f64 error is ~4096 * machine_epsilon ~ 9e-13, negligible.

### Lanczos3 Fast Path (`warp/mod.rs` lines 205-236)

The bounds check at line 205 (`kx0 >= 0 && ky0 >= 0 && kx0 + 5 < iw && ky0 + 5 < ih`)
correctly identifies when the full 6x6 kernel window is within bounds. The `+ 5` offset
is correct because the kernel spans indices `[kx0, kx0+5]` inclusive (6 pixels).

**Performance:** The fast path uses `get_unchecked` for direct indexing, avoiding 36
bounds checks per pixel. Row offsets are computed once per kernel row. This is a meaningful
optimization for interior pixels (the vast majority of a typical astronomical image).

**Correctness concern -- resolved:** The optimized Lanczos3 uses `kx0 = x0 - 2` (line
157), while the generic `interpolate_lanczos_impl` uses `x0 - a_i32 + 1` (line 220 in
mod.rs). For A=3: `x0 - 3 + 1 = x0 - 2`. These match.

### SIMD Bilinear Dispatch (`warp/mod.rs` lines 26-51)

Dispatch chain:
1. If no SIP and width >= 8 and has AVX2 -> AVX2 path
2. Else if no SIP and width >= 4 and has SSE4.1 -> SSE4.1 path
3. Else -> scalar fallback

**Correctness:** SIP is correctly excluded from SIMD paths because SIMD assumes linear
coordinate mapping (the SIMD code directly uses the transform matrix, not `wt.apply()`).

**Issue:** The SIMD functions in `sse.rs` take `&Transform` directly (not `&WarpTransform`),
bypassing SIP even if it were present. The dispatch in `warp_row_bilinear` guards this
with `!wt.has_sip()` (line 33), so there is no bug, but the SIMD functions could be called
incorrectly if someone bypasses the dispatch.

**Missing:** The dispatch checks `has_avx2()` but the AVX2 function is tagged with only
`#[target_feature(enable = "avx2")]`, not `"avx2,fma"`. The code does not use FMA
instructions, so this is fine, but FMA could improve accuracy of the bilinear computation.

## SIMD Analysis (`warp/sse.rs`)

### AVX2 Bilinear (lines 20-152)

**Architecture:** Processes 8 pixels per iteration. For each chunk:
1. Compute source coordinates for 8 output pixels using SIMD (lines 79-88)
2. Store coordinates to arrays, sample 4 neighbors per pixel in scalar loop (lines 100-126)
3. Perform bilinear blending in SIMD (lines 128-142)

**Key bottleneck:** Step 2 is entirely scalar -- 8 iterations of `sample_pixel` per corner,
= 32 scalar memory lookups per 8-pixel chunk. This is the dominant cost.

**Why no SIMD gather?** AVX2 has `_mm256_i32gather_ps` which could load scattered f32
values. However:
- Gather has high latency (~12-20 cycles on Haswell/Skylake)
- Each pixel needs 4 gathers (4 corners), so 4 gathers = ~48-80 cycles
- The scalar approach does 32 loads but can pipeline them
- Research confirms gather is often slower for small scattered loads (see IEEE paper on
  AVX2 gather for image processing)
- The scalar fallback for pixel sampling is a pragmatic choice

**Potential improvement:** For interior pixels where all coordinates are in-bounds, the
sample_pixel bounds check can be skipped entirely. A hybrid approach: check if the whole
8-pixel chunk maps to interior coordinates, then use direct unchecked loads. This would
remove 32 branch misprediction opportunities per chunk.

**Transform precision:** The SIMD code converts the f64 transform matrix to f32 once per
row (lines 36-42). For a 4096-wide image at f32 precision, the worst-case coordinate error
from f32 truncation is ~4096 * 1.2e-7 = ~0.0005 pixels. This is acceptable for bilinear
(which interpolates over 1-pixel neighborhoods) but could cause visible artifacts for
higher-order interpolation if the SIMD approach were extended to Lanczos.

### SSE4.1 Bilinear (lines 162-277)

Same algorithm as AVX2 but processes 4 pixels at a time. Uses `_mm_floor_ps` (SSE4.1
instruction, hence the feature requirement). Otherwise identical structure.

### Lanczos3: Explicit FMA SIMD Kernel (`warp/sse.rs`)

`lanczos3_kernel_fma<const DERINGING: bool>` processes the 6x6 kernel accumulation using
SSE FMA intrinsics. LLVM does NOT auto-vectorize the 6-element inner loop (assembly
analysis confirmed purely scalar `mulss`/`addss`).

**Architecture:** Pre-loads `wx[0..3]` and `wx[4..5]` into two `__m128` registers. For
each of the 6 rows: loads 8 source pixels as two `__m128` vectors, multiplies by `wx`,
then uses `_mm_fmadd_ps` to accumulate `weighted * wy[j]`. Horizontal sum via
`movehdup + add + movehl + add_ss`.

**Deringing fusion:** When `DERINGING=true`, tracks min/max using `_mm_min_ps`/`_mm_max_ps`
on the same loaded source vectors, fusing min/max tracking with accumulation in a single
pass. This avoids a separate 36-element scan, providing the biggest speedup for deringing.

**Results (1024x1024, single-threaded, correct affine transform):**
- Deringing: 80.9ms (scalar) → 33.4ms (FMA) = **-59%**
- No deringing: 36.6ms → 30.3ms = **-17%**
- Multi-threaded 4k: 95.7ms → 45.7ms = **-52%**

**Bounds requirement:** `kx0 + 8 < input_width` (reads 8 floats per row via two
`_mm_loadu_ps`; lanes 6-7 are garbage but zeroed by `wx_hi = [wx4, wx5, 0, 0]`).

**Dispatch:** Runtime check via `cpu_features::has_avx2_fma()`, cached outside pixel loop.
Falls back to scalar fast path for edge pixels (only needs `kx0 + 5 < iw`).

**Note:** Cannot be inlined into the caller because `#[target_feature(enable = "avx2,fma")]`
functions can only be inlined into functions with the same target features. The per-pixel
call overhead is measurable but small compared to the SIMD savings.

**Previous incorrect claim:** NOTES-AI previously stated LLVM auto-vectorizes the scalar
loop — assembly analysis proved this wrong. Only scalar `mulss`/`addss` is generated.

## Performance Analysis

### Parallelization (`mod.rs` lines 258-274)

Row-level parallelism via rayon `par_chunks_mut`. Each row is processed independently.
This is efficient:
- Output rows are contiguous in memory (good write pattern)
- Input access is scattered but cache-friendly for small rotations (adjacent output rows
  access nearby input rows)
- No synchronization needed between rows

**Potential issue with large rotations:** If the transform involves a 90-degree rotation,
consecutive output rows map to consecutive input columns, causing cache-unfriendly stride-1
access across the full image width. Tile-based processing could help for these cases but
adds complexity.

### Memory Access Patterns

**Current approach (row-at-a-time):**
- Output: sequential writes, optimal for cache
- Input: depends on transform. For typical astro registration (small rotation + translation),
  input access is nearly sequential -- adjacent output pixels map to nearby input pixels.
  Cache lines are reused efficiently.

**Alternative (tile-based):**
- Would improve input cache reuse for large rotations/scaling
- Adds code complexity
- For typical astrophotography (< 10 degree rotation, ~1x scale), row-based is fine

**Conclusion:** Row-based is appropriate for the use case.

### LUT Cache Behavior

Lanczos3 LUT is 48 KB. L1 data cache is typically 32-48 KB. The LUT may just barely fit
in L1. During Lanczos3 interpolation, each pixel does 12 LUT lookups at pseudo-random
offsets. If the LUT is fully cached, lookups are ~4 cycles each. If it spills to L2,
lookups are ~12 cycles each. The 48 KB size is borderline.

**Optimization option:** Reduce LUT resolution to 2048 samples/unit (24 KB, fits L1
comfortably). Error would increase from ~0.00043 to ~0.00086, still well within f32
precision. Alternatively, use linear interpolation with 1024 samples/unit (12 KB) for
similar precision.

## Issues Found

### 1. No Anti-Aliasing Prefilter for Downscaling (Low - Context-Dependent)
When the transform involves scaling down (scale factor < 1), the Nyquist frequency of the
output is lower than the input. Without prefiltering, high-frequency content aliases into
the output. The current code does no prefiltering.

**For astrophotography:** This is generally not an issue because:
- Registration typically maps images at ~1:1 scale
- Astronomical images are already band-limited by the PSF (seeing/optics)
- Small scale differences (0.95x-1.05x) produce minimal aliasing

However, if this module is used for image rescaling (e.g., drizzle output downscaling or
mosaic edge matching with different plate scales), aliasing could appear.

**What others do:**
- PixInsight: no explicit mention of anti-aliasing prefilter in their interpolation docs
- Siril: offers an `area` interpolation option for downsampling
- OpenCV: `INTER_AREA` for downscaling, but `warpAffine` with `INTER_LINEAR` does NOT
  prefilter (known issue #21060 on GitHub)
- Intel IPP: uses separable approach that inherently handles this for fixed ratios

### 2. Bicubic Not Normalized at Boundaries (Low)
At image boundaries, `sample_pixel` returns 0.0 for out-of-bounds coordinates. For
bicubic interpolation, this means the effective kernel weights don't sum to 1.0 near
edges, producing darkened pixels. The same applies to bilinear. This matches zero-padding
border behavior and is expected for astrophotography (borders are cropped), but differs
from clamp-to-edge which would be more correct.

## Missing Features / Potential Improvements (Prioritized)

### High Priority
1. **Separable Lanczos3:** Two-pass (horizontal then vertical) reduces 36 ops to 12 per
   pixel. Each pass processes consecutive pixels enabling natural SIMD vectorization. Intel
   IPP uses this approach. Note: separable decomposition does not directly apply to
   arbitrary warps with rotation (source coordinates differ per pixel), but shear
   decomposition of affine transforms could enable it.

### Postponed (low impact)
- **SIMD bilinear interior fast path** — skip bounds checks for interior chunk. Marginal.
- **FMA for SIMD bilinear** — replace `mul+add` with `fmadd`. Marginal.
- **Inline FMA kernel** — ~2-3ms gain out of 33ms. Not worth the complexity.
- **Linear interpolation in LUT** — current 4096 samples/unit is adequate for f32.
- **Tile-based processing** — only helps extreme rotations (>45°), rare in practice.
- **Configurable border modes** — borders are cropped in astrophotography.

### Tried and Rejected
- **AVX2 gather for LUT lookups:** `_mm256_i32gather_ps` for 6 weights at once. ~2% slower
  than scalar lookups due to high gather latency (~12 cycles) vs L1-cached scalar loads
  (~4 cycles). The 48KB LUT fits in L1 cache, making scalar lookups fast enough.
- **Per-pixel separable factorization:** Restructuring inner loop from `v*wx*wy` to
  row_sums then vertical combine. No improvement — LLVM already optimizes the original
  pattern.

## References

- [PixInsight Interpolation Algorithms](https://pixinsight.com/doc/docs/InterpolationAlgorithms/InterpolationAlgorithms.html) -- Detailed analysis of Lanczos, bicubic, Mitchell-Netravali for astrophotography. Recommends Lanczos-3 with clamping threshold 0.3.
- [Intel IPP AVX Lanczos](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html) -- Two-pass separable Lanczos3 with AVX SIMD. 42 mults + 35 adds per pixel. 1.5x gain from SSE to AVX.
- [AVIR Lanczos Resizer](https://github.com/avaneev/avir) -- High-quality SIMD Lanczos resizer with AVX2/SSE2/NEON. LANCIR variant is 3x faster.
- [Lanczos Interpolation Explained](https://mazzo.li/posts/lanczos.html) -- Clear explanation of Lanczos kernel, ringing, and normalization.
- [IEEE: AVX2 Gather for Image Processing](https://ieeexplore.ieee.org/document/8634707/) -- Analysis showing gather instructions have mixed performance for scattered image access.
- [OpenCV warpAffine Source](https://github.com/opencv/opencv/blob/master/modules/imgproc/src/imgwarp.cpp) -- Reference implementation with SIMD coordinate computation, blockline processing.
- [Siril Registration Docs](https://siril.readthedocs.io/en/latest/preprocessing/registration.html) -- Uses Lanczos-4 by default with clamping option.
- [Mitchell-Netravali Filters (Wikipedia)](https://en.wikipedia.org/wiki/Mitchell%E2%80%93Netravali_filters) -- B/C parameter space. Catmull-Rom is (B=0, C=0.5), Mitchell is (B=1/3, C=1/3).
- [Bart Wronski: Bilinear Downsampling Pixel Grids](https://bartwronski.com/2021/02/15/bilinear-down-upsampling-pixel-grids-and-that-half-pixel-offset/) -- Explains anti-aliasing requirements for downscaling.
- [AstroPiXelProcessor: Interpolation Artifacts](https://www.astropixelprocessor.com/community/tutorials-workflows/interpolation-artifacts/) -- Practical examples of Lanczos ringing around stars.
