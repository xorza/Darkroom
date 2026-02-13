# Background Estimation Module -- Analysis Notes

## Module Overview

Tile-based background and noise estimation for astronomical images. Divides image into tiles, computes sigma-clipped median + MAD per tile, applies 3x3 median filter to tiles, then bilinearly interpolates per-pixel background and noise maps. Supports iterative refinement with source masking.

## File Map

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | 252 | Entry points (`estimate_background`, `refine_background`), row interpolation, object masking |
| `estimate.rs` | 27 | `BackgroundEstimate` struct (background + noise buffers) |
| `tile_grid.rs` | 1233 | `TileGrid` struct, sigma-clipped stats, 3x3 median filter, pixel collection, unit tests |
| `simd/mod.rs` | 823 | Runtime SIMD dispatch, `interpolate_segment_simd`, `sum_and_sum_sq_simd`, scalar fallbacks, tests |
| `simd/sse.rs` | 275 | SSE4.1 + AVX2 implementations for sum/sum_sq, sum_abs_deviations |
| `simd/neon.rs` | 124 | ARM NEON implementations for sum/sum_sq, sum_abs_deviations |
| `tests.rs` | 878 | Integration tests for full pipeline, sigma-clipped statistics |
| `bench.rs` | 73 | Benchmarks: 6K full estimate, tile grid with/without mask |

Statistics core lives externally in `lumos/src/math/statistics/mod.rs` (lines 174-196): `sigma_clipped_median_mad()`.

## Algorithm Pipeline

```
estimate_background (mod.rs:29)
  |
  +-> TileGrid::new_uninit (tile_grid.rs:43)
  +-> TileGrid::compute (tile_grid.rs:56)
  |     +-> fill_tile_stats (tile_grid.rs:129)  -- parallel per-tile
  |     |     +-> collect pixels (all/sampled/unmasked)
  |     |     +-> sigma_clipped_median_mad (math/statistics/mod.rs:174)
  |     +-> apply_median_filter (tile_grid.rs:178)  -- 3x3 median on tile grid
  |
  +-> interpolate_from_grid (mod.rs:105)  -- parallel per-row
        +-> interpolate_row (mod.rs:151)  -- segment-based bilinear
              +-> simd::interpolate_segment_simd (simd/mod.rs:108)

refine_background (mod.rs:59)  -- optional iterative refinement
  for each iteration:
    +-> create_object_mask (mod.rs:121)
    |     +-> threshold_mask::create_threshold_mask
    |     +-> mask_dilation::dilate_mask
    +-> TileGrid::compute (with mask)
    +-> interpolate_from_grid
```

## Comparison with Industry Standards

### Feature-by-Feature Comparison

| Feature | SExtractor | SEP (C library) | photutils Background2D | This Module |
|---------|------------|------------------|------------------------|-------------|
| **Tile/mesh size** | BACK_SIZE (32-512) | bw, bh (default 64) | box_size (default varies) | tile_size (16-256, default 64) |
| **Tile statistic** | Mode: 2.5*med - 1.5*mean | Same as SExtractor | Pluggable (6 estimators) | Median only |
| **Scale estimator** | Clipped std dev | Same as SExtractor | Pluggable (3 estimators) | MAD * 1.4826 |
| **Sigma clipping** | 3-sigma, iterate to convergence | Same as SExtractor | sigma=3, maxiters=10 | kappa=3, 2-3 iterations |
| **Tile filter** | 3x3 median (configurable) | fw, fh (default 3x3) | filter_size (default 3x3) | 3x3 median (fixed) |
| **Interpolation** | Bicubic spline | Bicubic spline | BkgZoomInterpolator (spline) | Bilinear |
| **Edge handling** | Partial meshes supported | Same as SExtractor | edge_method param | Partial tiles with adjusted centers |
| **Source masking** | Automatic via mode estimator | Same | External mask parameter | Iterative refinement with mask |
| **RMS map** | sigma from clipped std dev | Same | Separate RMS estimator | MAD-based sigma per tile |
| **SIMD** | No (scalar C) | No (scalar C) | No (Python/NumPy) | AVX2, SSE4.1, NEON |
| **Parallelism** | No (single-threaded) | No | NumPy vectorized | Rayon parallel (per-tile, per-row) |

### Detailed Analysis

#### 1. Tile Statistics: Mode vs Median

**SExtractor/SEP** use a mode estimator: `Mode = 2.5 * Median - 1.5 * Mean`. This is specifically designed for crowded fields where sources contaminate tiles. The mode of a positively skewed distribution (sky + faint sources) is lower than the median, giving a better background estimate. SExtractor falls back to median when mode and median disagree by >30%.

**This module** uses pure median (tile_grid.rs:269, via `sigma_clipped_median_mad`). The median is robust to outliers but slightly biased upward in crowded fields compared to the mode estimator. SExtractor docs note the mode is "considerably less affected by source crowding than a simple clipped mean" but "approximately 30% noisier."

**photutils** offers pluggable estimators including `SExtractorBackground`, `MMMBackground`, `BiweightLocationBackground`, and simple `MedianBackground`. The biweight location is the most statistically robust option.

**Impact**: In sparse fields, median is equivalent to mode. In crowded fields (globular clusters, galaxy cores), the mode estimator gives ~1-5% lower background values, which matters for faint source photometry. The iterative refinement in this module partially compensates by masking detected sources before re-estimation.

#### 2. Scale Estimator: MAD vs Std Dev

**SExtractor** uses clipped standard deviation for the noise (RMS) map.

**This module** uses MAD * 1.4826 (math/statistics/mod.rs:17, `MAD_TO_SIGMA` constant). MAD is more robust than std dev -- it has a breakdown point of 50% (can tolerate up to 50% contaminated data) vs ~0% for std dev. The 1.4826 factor makes it consistent with sigma for Gaussian distributions.

**photutils** offers `StdBackgroundRMS`, `MADStdBackgroundRMS`, and `BiweightScaleBackgroundRMS`.

**Verdict**: MAD is a better choice than std dev for astronomical backgrounds. This is a strength of the current implementation.

#### 3. Sigma Clipping: Convergence

**SExtractor** clips at +-3-sigma and iterates until convergence (no fixed iteration count).

**This module** uses a fixed iteration count (default 3, configurable via `sigma_clip_iterations` in config.rs:268). With MAD-based sigma, convergence is faster than with std dev -- typically 2-3 iterations suffice. The implementation in `sigma_clip_iteration` (math/statistics/mod.rs:95) stops early if sigma drops to zero or no values are clipped, which provides implicit convergence detection.

**photutils** defaults to sigma=3, maxiters=10 with explicit convergence checking.

**Verdict**: Fixed iteration count with MAD is a reasonable approach. The early-exit on convergence (math/statistics/mod.rs:136-139) provides the same effect as convergence-based stopping.

#### 4. Interpolation: Bilinear vs Bicubic Spline

**SExtractor/SEP** use natural bicubic spline interpolation between mesh points. This produces C1-continuous surfaces (continuous first derivatives).

**This module** uses bilinear interpolation (mod.rs:151-231). Bilinear produces C0-continuous surfaces (continuous values but discontinuous derivatives at tile boundaries).

**photutils** defaults to `BkgZoomInterpolator` (spline) but also offers `BkgIDWInterpolator` (inverse distance weighting).

**Impact**: Bicubic spline provides smoother interpolation that better handles strong gradients. The difference is most visible at tile boundaries where bilinear creates subtle discontinuities in the derivative. However, the 3x3 median filter on tiles smooths out the most problematic tile-to-tile variations. For typical astronomical images with smooth backgrounds, the practical difference is small. The test `test_interpolation_smooth_at_tile_boundaries` (tests.rs:340) verifies continuity with a max_jump of 0.05.

**Performance trade-off**: Bilinear is significantly cheaper (2 multiplies + 2 adds per dimension) vs bicubic (requires solving 4x4 system per tile). The SIMD-accelerated bilinear (simd/mod.rs:211-299) processes 8 pixels per AVX2 iteration.

#### 5. Tile Filter

**SExtractor** uses a configurable median filter (BACK_FILTERSIZE, default 3x3) with a threshold parameter (BACK_FILTERTHRESH) for differential filtering.

**This module** applies a fixed 3x3 median filter (tile_grid.rs:178-219). Skipped for grids smaller than 3x3 (tile_grid.rs:182-184). No threshold parameter.

**Missing**: The ability to skip filtering or use different filter sizes. SExtractor's `BACK_FILTERTHRESH` allows selective filtering where only tiles with values below a threshold are filtered, preserving the structure of bright regions.

#### 6. Sampling Strategy

**SExtractor** processes all pixels in each mesh.

**This module** caps at MAX_TILE_SAMPLES=1024 (tile_grid.rs:13) using 2D strided sampling (tile_grid.rs:294-314). For 64x64 tiles (4096 pixels), this is ~4x faster. For 128x128 tiles (16384 pixels), ~16x faster.

**Trade-off**: With ~1024 samples, median accuracy is within 1-2% (per TILE_GRID.md). This is a good engineering choice that SExtractor does not make.

#### 7. Mask Handling

**SExtractor** does not accept external masks for background estimation; it relies on the mode estimator to handle source contamination.

**This module** supports bit-packed masks (tile_grid.rs:319-371) with efficient word-level collection (`collect_unmasked_pixels`). Falls back to all pixels when too few unmasked (tile_grid.rs:249-251). The refinement loop (mod.rs:79-98) creates masks from detected sources, dilates them, and re-estimates.

**photutils** accepts external masks and coverage masks.

**Verdict**: The iterative refinement with automatic source masking is more sophisticated than SExtractor's approach and matches photutils' recommendation.

#### 8. SIMD Acceleration

**No other tool** (SExtractor, SEP, photutils) uses SIMD for background estimation.

This module provides SIMD for:
- **Bilinear interpolation**: AVX2 (8 floats), SSE4.1 (4 floats), NEON (4 floats) -- simd/mod.rs:211-418
- **Sum + sum-of-squares**: AVX2, SSE4.1, NEON -- simd/sse.rs:14-105, simd/neon.rs:13-45
- **Sum of absolute deviations**: AVX2, SSE4.1, NEON -- simd/sse.rs:125-189, simd/neon.rs:52-79

Note: `sum_and_sum_sq_simd` and `sum_abs_deviations_simd` are currently marked `#[allow(dead_code)]` (simd/mod.rs:21,46) -- they exist for future use but are not called by the tile statistics pipeline. The actual sigma-clipped statistics use scalar code in `math/statistics/mod.rs`.

#### 9. Edge Handling

**SExtractor** allows partial edge meshes.

**This module** handles partial tiles naturally via `div_ceil` for tile count (tile_grid.rs:44-45) and clamped tile extents (tile_grid.rs:159-160). Tile centers are adjusted for partial tiles (tile_grid.rs:90-101), and the test at tile_grid.rs:477-481 verifies this.

**photutils** has an explicit `edge_method` parameter.

## Issues and Gaps

### 1. No Mode/Biweight Location Estimator for Crowded Fields

The module uses pure median. In crowded fields (globular clusters, galaxy cores), sources bias the median upward. Two better estimators exist:

**SExtractor mode**: `Mode = 2.5*Median - 1.5*Mean` after sigma clipping. Less affected
by source crowding (~30% noisier but lower bias). Falls back to median when mode/median
disagree by >30%. Simple to implement (~5 lines in `compute_tile_stats`).

**Biweight location** (Tukey 1977): Iteratively down-weights outliers using a smooth
weight function. Combines the robustness of the median (50% breakdown point) with
near-Gaussian efficiency (98.2% vs median's 64%). Used by photutils as the recommended
`BiweightLocationBackground` estimator. More complex to implement but statistically
superior. The formula: `BL = M + sum(u_i * (x_i - M) * (1 - u_i^2)^2) / sum((1 - u_i^2)^2)`
where `u_i = (x_i - M) / (c * MAD)` and `c = 9.0` (tuning constant).

**Severity**: Medium. Primarily affects crowded field photometry accuracy. The iterative
refinement with source masking partially compensates.

**Recommendation**: For highest impact with least effort, implement SExtractor mode first.
Biweight location is a future enhancement for photometry-grade accuracy.

### 2. Bilinear Interpolation (vs Bicubic Spline)

Bilinear interpolation produces C0 surfaces with derivative discontinuities at tile boundaries. SExtractor and SEP both use bicubic spline for smoother results.

**Severity**: Low-Medium. The 3x3 median filter mitigates the worst artifacts. With default 64px tiles on typical images, the difference is minor. More noticeable with large tile sizes or strong gradients.

**Fix**: Implement natural bicubic spline interpolation. Would require storing 4x4 tile neighborhoods instead of 2x2. Higher computational cost but could still be SIMD-accelerated.

### 3. Fixed 3x3 Median Filter

The filter size is hardcoded (tile_grid.rs:182). SExtractor allows 1x1 (none), 3x3, 5x5, or arbitrary sizes via BACK_FILTERSIZE. No threshold parameter.

**Severity**: Low. 3x3 is the recommended default in SExtractor and sufficient for most cases.

**Fix**: Add `tile_filter_size` to Config. Currently not wired but straightforward.

### 4. Dead SIMD Code for Statistics

`sum_and_sum_sq_simd` and `sum_abs_deviations_simd` (simd/mod.rs:22,48) are implemented but unused. The actual per-tile sigma clipping uses scalar `median_f32_approx` + `abs_deviation_inplace` in math/statistics/mod.rs. SIMD acceleration of the sort-based median would be more impactful.

**Severity**: Low. The statistics computation is fast enough due to MAX_TILE_SAMPLES=1024 cap. The bottleneck is median computation (quickselect), not sum/deviation.

### 5. No Configurable Clipping Sigma

The clipping sigma is hardcoded to 3.0 in `compute_tile_stats` (tile_grid.rs:269). While 3-sigma is standard, some use cases (very crowded or nebulous fields) benefit from tighter clipping (2-sigma).

**Severity**: Low. 3-sigma is the industry standard. Could be exposed as a config parameter if needed.

### 6. No Weight Map Support

The module does not support input weight or variance maps. Some astronomical images have spatially varying noise (e.g., mosaics with different exposure times, or images with bad columns).

**Severity**: Low for typical astrophotography. Higher for survey data.

## Strengths vs Industry

1. **MAD-based sigma** is more robust than SExtractor's clipped std dev (tile_grid.rs:269, math/statistics/mod.rs:174-196)
2. **SIMD-accelerated interpolation** with AVX2/SSE4.1/NEON -- unique among background estimators
3. **Rayon parallelism** for both tile stats (tile_grid.rs:142-175) and row interpolation (mod.rs:108-116)
4. **Iterative refinement** with source masking + dilation (mod.rs:59-102) matches photutils best practices
5. **Bit-packed mask collection** with word-level operations (tile_grid.rs:319-371) is very efficient
6. **Sampling optimization** caps at 1024 samples/tile (tile_grid.rs:13) -- SExtractor processes all pixels
7. **Comprehensive test coverage**: 48+ tile_grid tests, 30+ integration tests, SIMD correctness tests
8. **Buffer pool** integration for zero-allocation repeated calls (mod.rs:32-33, estimate.rs:22-25)

## Performance

Benchmarks on 6144x6144 globular cluster (50,000 stars), from bench.rs:

| Operation | Time |
|-----------|------|
| Full background estimate (6K) | ~100ms |
| TileGrid (no mask) | ~45ms |
| TileGrid (with mask) | ~32ms |

The masked case is faster because fewer pixels are collected (word-level bit operations skip masked regions).

## Configuration Defaults (config.rs:263-271)

```
tile_size: 64
sigma_clip_iterations: 3
refinement: BackgroundRefinement::None
bg_mask_dilation: 3
min_unmasked_fraction: 0.3
sigma_threshold: 4.0
```

## References

- [SExtractor Background Modeling](https://sextractor.readthedocs.io/en/latest/Background.html) -- canonical reference for mesh-based estimation
- [SEP C Library](https://sep.readthedocs.io/en/latest/) -- standalone C implementation of SExtractor algorithms
- [photutils Background2D](https://photutils.readthedocs.io/en/stable/user_guide/background.html) -- Python/astropy framework with pluggable estimators
- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html) -- sigma clipping reference implementation
- [GNU Astronomy Utilities: Sigma clipping](https://www.gnu.org/software/gnuastro/manual/html_node/Sigma-clipping.html) -- convergence-based sigma clipping
- [PixInsight DBE/ABE](https://pixinsight.com/doc/docs/InterpolationAlgorithms/InterpolationAlgorithms.html) -- polynomial/spline background models
- Bertin & Arnouts (1996), "SExtractor: Software for source extraction", A&AS 117, 393
