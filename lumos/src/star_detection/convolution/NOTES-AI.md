# convolution/ Module - Implementation Notes

## Overview

Separable Gaussian convolution for matched-filter star detection. Convolves a
background-subtracted image with a Gaussian kernel matching the expected PSF to boost
SNR for faint point sources. Supports circular (separable, O(n*k)) and elliptical
(full 2D, O(n*k^2)) PSFs. SIMD-accelerated on x86_64 (AVX2+FMA, SSE4.1) and aarch64
(NEON).

## File Structure

| File | Lines | Purpose |
|------|-------|---------|
| mod.rs | 325 | Public API, kernel generation, separable/2D convolution orchestration |
| simd/mod.rs | 268 | SIMD dispatch, scalar fallbacks, mirror_index, 1D/2D row+col functions |
| simd/sse.rs | 563 | AVX2+FMA and SSE4.1 implementations for row, column, and 2D convolution |
| simd/neon.rs | 232 | ARM NEON implementations (row, column, 2D convolution) |
| simd/tests.rs | 741 | SIMD vs scalar correctness tests (row, column, 2D, mirror_index) |
| tests.rs | 827 | Integration tests: kernel, convolution, matched filter, elliptical, numerics |
| bench.rs | 274 | Benchmarks: row SIMD/scalar, column, full image, elliptical, matched filter |

## Public API

### `matched_filter()` (mod.rs:45-79)
Entry point for the detection pipeline. Steps:
1. **Background subtraction** (line 67-74): `(pixels - background).max(0.0)` in parallel.
   Clips negative values to zero.
2. **FWHM to sigma** (line 77): `sigma = fwhm / 2.35482`
3. **Convolution** (line 78): Dispatches to `elliptical_gaussian_convolve()`.

Parameters: `pixels`, `background`, `fwhm`, `axis_ratio` (1.0=circular), `angle` (radians),
`output`, `subtraction_scratch`, `temp`.

### `gaussian_convolve()` (mod.rs:89-117)
Separable convolution for circular Gaussians. Two passes:
1. Horizontal (row) pass: `convolve_rows_parallel()` -> `temp` buffer
2. Vertical (column) pass: `convolve_cols_parallel()` -> `output` buffer

Falls back to direct 2D if kernel radius >= `min(width, height) / 2` (line 107).

### `elliptical_gaussian_convolve()` (mod.rs:124-163)
For non-circular PSFs. Dispatches:
- `axis_ratio >= 0.99` -> uses separable `gaussian_convolve()` (line 138-141)
- Otherwise -> full 2D convolution with `simd::convolve_2d_row()` per row in parallel

## Kernel Design

### 1D Gaussian Kernel (mod.rs:166-188)
- Radius: `ceil(3 * sigma)` pixels (line 169). For sigma=2.0, radius=6, size=13.
- Normalization: **Sum = 1.0** (line 183-185). Preserves flux.
- Shape: Pure Gaussian, no negative wings.

### 2D Elliptical Kernel (mod.rs:280-324)
- Same 3-sigma truncation radius based on major axis sigma (line 287).
- Rotated coordinates: `x_rot = x*cos + y*sin`, `y_rot = -x*sin + y*cos` (lines 308-309).
- Two sigmas: `sigma_major = sigma`, `sigma_minor = sigma * axis_ratio` (lines 290-291).
- Normalization: **Sum = 1.0** (line 319-321). Preserves flux.

## SIMD Architecture

### Dispatch Hierarchy (simd/mod.rs:80-108, 133-168, 198-239)
Three dispatch points, each with runtime CPU detection:
1. **Row convolution**: AVX2+FMA (8 pixels/iter) -> SSE4.1 (4 pixels/iter) -> scalar
2. **Column convolution**: Same dispatch chain, processes row-by-row for cache locality
3. **2D convolution**: Same dispatch chain, per-row processing

### Row Convolution - AVX2 (simd/sse.rs:20-71)
- Processes 8 pixels per iteration with `_mm256_fmadd_ps`.
- Safe region: `[radius .. width - radius - 7]` for direct loads.
- Edge pixels (left+right boundaries) handled by scalar fallback with mirror indexing.
- Small inputs (< 16 + 2*radius): full scalar fallback (line 26).

### Row Convolution - SSE4.1 (simd/sse.rs:79-130)
- Same structure, 4 pixels per iteration with `_mm_mul_ps` + `_mm_add_ps` (no FMA).

### Column Convolution - AVX2 (simd/sse.rs:140-185)
- Row-major traversal: iterates y then x for cache locality.
- Loads 8 columns at once from each kernel-offset row.
- Mirror boundary for out-of-bounds y indices via `mirror_index()`.

### Column Convolution - SSE4.1 (simd/sse.rs:194-239)
- Same structure, 4 columns per iteration.

### 2D Convolution - AVX2 (simd/sse.rs:249-319)
- Per-row function, called in parallel from `elliptical_gaussian_convolve()`.
- Processes 8 output pixels at a time.
- Skips kernel elements with `|kval| < 1e-10` (line 274) - optimizes sparse elliptical kernels.
- Fast path: if 8-pixel load window is in-bounds, direct `_mm256_loadu_ps` (line 281-285).
- Slow path: gather with `mirror_index()` into stack array, then load (lines 287-294).

### NEON (simd/neon.rs)
- Mirrors SSE4.1 structure but with 4 pixels/iter using `vfmaq_f32` (has FMA).
- Row (lines 15-67), column (lines 75-120), 2D (lines 129-198).

### Scalar Fallbacks (simd/mod.rs:112-123, 172-191, 244-267)
- Used on platforms without SIMD or for edge pixels.
- `convolve_pixel_scalar()` (lines 54-70): inner loop for single pixel with mirror boundary.
- `convolve_cols_scalar()` (lines 172-191): column-first traversal (poor cache locality, OK for fallback).

## Edge Handling: Mirror Boundary

### `mirror_index()` (simd/mod.rs:36-50)
- Reflects indices at boundaries: `-1 -> 1`, `-2 -> 2`, `len -> len-2`, `len+1 -> len-3`.
- Clamps reflected indices that are still out of bounds (far overshoot).
- Used consistently in all scalar and SIMD boundary paths.

## Parallelism

- **Row pass** (mod.rs:196-212): `par_chunks_mut(width)` - each row is independent.
- **Column pass** (mod.rs:219-236): Single-threaded SIMD (processes row-by-row for locality).
- **2D elliptical** (mod.rs:147-162): `par_chunks_mut(width)` - each output row independent.
- **Background subtraction** (mod.rs:67-74): `par_iter_mut().zip()`.

## Comparison With Industry Standards

### vs SExtractor (Bertin & Arnouts 1996)

| Aspect | SExtractor | This Implementation |
|--------|-----------|-------------------|
| Kernel shape | User-supplied (default: 3x3 pyramidal, or Gaussian .conv files) | Gaussian only (1D separable or 2D elliptical) |
| Kernel normalization | Sum = 1.0 for Gaussian filters | Sum = 1.0 (same) |
| Negative wings | No (for standard conv filters) | No (same) |
| Separable | No (always 2D convolution) | Yes for circular, 2D for elliptical |
| Edge handling | Zero-pad ("virtual pixels set to zero") | Mirror/reflect boundary |
| Noise after conv | **Convolves variance map with squared kernel** | **Does not scale noise map (BUG - see Issues)** |
| Filter flexibility | Any user-supplied filter file | Fixed Gaussian shape |

**Key difference**: SExtractor convolves the variance map alongside the image to maintain
correct noise scaling. This implementation uses the unconvolved noise map for thresholding,
which makes the effective threshold ~6-7x too high for typical FWHM values. See P1 issue
in parent NOTES-AI.md.

### vs DAOFIND / photutils DAOStarFinder (Stetson 1987)

| Aspect | DAOFIND | This Implementation |
|--------|---------|-------------------|
| Kernel shape | Lowered Gaussian (zero-sum, negative wings) | Pure Gaussian (sum=1, no negative wings) |
| Background handling | Kernel implicitly subtracts local background | Explicit background subtraction before convolution |
| Noise correction | `threshold_eff = threshold * kernel.relerr` where `relerr = 1/sqrt(sum(K^2) - sum(K)^2/N)` | **No noise correction (BUG)** |
| Edge handling | Zero-pad | Mirror/reflect |
| Convolution output | Density enhancement (fits Gaussian amplitude) | Smoothed image (preserves flux) |
| Separable | No (2D kernel with mask) | Yes for circular |

**Key difference**: DAOFIND uses a zero-sum kernel that subtracts local background
during convolution and normalizes the output so that threshold is in standard-deviation
units. This implementation separates background subtraction from convolution, which is
valid but requires proper noise scaling (which is currently missing).

### vs SEP (SExtractor in Python, Barbary 2016)

SEP implements the full matched filter: `T = C^{-1}S / sqrt(S^T C^{-1} S)`, where C
is the noise covariance matrix and S is the PSF kernel. For uniform noise, this reduces
to simple convolution divided by `sqrt(sum(S^2))`. The output is in units of
standard deviations above background. This implementation does not perform this normalization.

### vs PixInsight StarDetection

PixInsight uses wavelet-based (a trous) multiscale structure detection rather than
matched filtering. The matched-filter approach here is mathematically equivalent to
PixInsight's correlation method for Gaussian PSFs but cannot handle scale variation
across the field.

## Design Decisions

### Mirror vs Zero-Pad Boundary
Mirror boundary (this implementation) is generally superior to zero-pad (SExtractor/DAOFIND)
for star detection:
- Zero-pad creates artificial dark borders that suppress real stars near edges.
- Mirror preserves local statistics at boundaries.
- Minor disadvantage: can create phantom reflections of bright stars at borders.

### Separable Decomposition
Correct for circular Gaussians only. The implementation properly falls back to full 2D
for elliptical kernels (axis_ratio < 0.99). Complexity reduction: O(n*k) vs O(n*k^2),
typically 5-10x faster for FWHM=4px (kernel size 13).

### Negative Value Clipping (mod.rs:73)
Background subtraction clips to zero: `(px - bg).max(0.0)`. This is non-standard:
- SExtractor does not clip negative residuals before convolution.
- Clipping removes negative noise fluctuations, biasing the convolved image upward.
- Impact: Faint stars near background level may have asymmetric noise response.
- However, for the purpose of detecting positive peaks, this is conservative and avoids
  negative-going artifacts in the convolved image.

## Known Issues

### P1: Noise Map Not Scaled After Convolution
- **Location**: Called from `detector/stages/detect.rs:95-98`
- After convolution with kernel K (sum=1), noise variance at each pixel becomes
  `sigma_conv^2 = sigma^2 * sum(K_i^2)`. The threshold mask compares `filtered[px]`
  against `sigma_threshold * noise[px]` using the **original** noise map.
- For a Gaussian kernel with FWHM=4px (sigma~1.7), `sqrt(sum(K^2))` ~ 0.11-0.15,
  so the effective sigma threshold is ~6-9x higher than configured.
- **Fix**: Either (a) convolve the variance map with K^2 and use `sqrt(convolved_var)`,
  or (b) multiply the threshold by `1/sqrt(sum(K^2))`, or (c) divide the convolved
  image by `sqrt(sum(K^2))` to normalize to noise units (SEP approach).

### P2: No Support for DAOFIND-style Zero-Sum Kernel
- The Gaussian kernel sums to 1.0, not 0.0. This means the convolution does not perform
  implicit local background subtraction. Background must be subtracted beforehand.
- Not necessarily a bug (explicit background subtraction is valid), but means the
  implementation cannot benefit from the local background subtraction that makes DAOFIND
  robust to background estimation errors.

### P3: Column Convolution Not Parallelized
- `convolve_cols_direct()` (simd/mod.rs:133-168 -> sse.rs:140-185) processes all rows
  sequentially in a single thread. Row convolution uses `par_chunks_mut()`.
- For large images (4K+), column pass could be a bottleneck.
- The row-major traversal is correct for cache locality but leaves multi-core unused.

### P3: 2D AVX2 Kernel Element Skip Threshold
- `convolve_2d_row_avx2` skips kernel elements with `|kval| < 1e-10` (sse.rs:274).
- This is effectively zero for f32 but the comparison itself adds a branch per kernel element.
- For dense kernels (circular Gaussian), nearly all elements are non-zero so this adds
  overhead without benefit. Only useful for very sparse elliptical kernels.

## Test Coverage

### Kernel Tests (tests.rs:12-76)
- Normalization (sum=1.0) for various sigma values
- Symmetry, peak at center, correct size
- Known Gaussian ratio at x=1: `G(1)/G(0) = exp(-0.5)`
- Panics on zero/negative sigma

### Convolution Tests (tests.rs:82-269)
- Uniform image preservation
- Total flux preservation (point source)
- Spreading of point source
- 4-fold symmetry from centered point source
- Larger sigma = more spreading
- Edge handling (no NaN/Inf, corner flux via mirror)
- Non-square images, small images
- Separable vs direct 2D equivalence

### Matched Filter Tests (tests.rs:275-425)
- Background subtraction (flat field -> ~0)
- Star detection (positive peak at star center)
- SNR boost (peak location correct with noise)
- Negative clipping (below-background -> non-negative output)

### Elliptical Tests (tests.rs:481-723)
- Kernel normalization, symmetry at angle=0, elongation verification
- Uniform image preservation, flux preservation
- axis_ratio=1.0 matches circular convolution
- Rotation invariance (same flux at 0 and 90 degrees, rotated spread)
- Various axis ratios (0.2 to 1.0)

### Numerical Tests (tests.rs:730-826)
- Linearity: conv(a + b) = conv(a) + conv(b)
- Scaling: conv(k*f) = k*conv(f)

### SIMD Tests (simd/tests.rs, simd/sse.rs:400-562, simd/neon.rs:200-231)
- Row: SIMD vs scalar for various sizes, kernels, edge cases, impulse response
- Column: SIMD vs scalar for various image sizes, uniform input, impulse response
- 2D: SIMD vs scalar for various kernel sizes, boundary rows, uniform/impulse
- Mirror index: in-bounds passthrough, negative reflection, overflow reflection, far OOB clamping
- Platform-specific: AVX2 vs scalar, SSE4.1 vs scalar, NEON vs scalar

## Performance Notes

Benchmarks in bench.rs measure:
- Row convolution SIMD vs scalar (4K width * 10 rows, sigma=2.0 and 5.0)
- Column convolution (1K and 4K images, sigma=2.0)
- Row vs column comparison (1K image)
- Full gaussian_convolve (1K and 4K images, sigma=2.0)
- Elliptical vs circular comparison (1K image)
- Matched filter full pipeline (1K and 4K, circular and elliptical)

Expected speedups: AVX2 row ~4-8x over scalar, separable ~5-10x over 2D for typical FWHM.
