# stacking Module

## Module Overview

Unified image stacking pipeline with six rejection algorithms, two combination methods,
three normalization modes, three weighting strategies (equal/noise/manual), and automatic
memory management.

### Files
- `mod.rs` - Public API, `FrameType` enum, re-exports
- `stack.rs` - `stack()`/`stack_with_progress()` entry points, normalization, dispatch
- `config.rs` - `StackConfig`, `CombineMethod`, `Normalization`, presets, validation
- `rejection.rs` - Six rejection algorithms with config structs, `Rejection` enum dispatch
- `cache.rs` - `ImageCache<I>` (in-memory or mmap), chunked parallel processing, `ScratchBuffers`
- `cache_config.rs` - `CacheConfig`, adaptive chunk sizing, system memory queries
- `error.rs` - `Error` enum (thiserror), I/O and dimension errors
- `progress.rs` - `ProgressCallback`, `StackingStage` (Loading/Processing)
- `bench.rs` - Stacking benchmarks for bias/dark/flat/light at 10/30/100 frames (1920x1280)
- `tests/real_data.rs` - Integration test stacking registered lights (ignored, requires calibration dir)

### Data Flow
1. `stack_with_progress()` validates config, creates `ImageCache` from paths
2. `ImageCache::from_paths()` loads images; picks in-memory (<75% RAM) or disk-backed (mmap)
3. `compute_norm_params()` derives per-frame per-channel `NormParams { gain, offset }`
4. `dispatch_stacking()` calls `cache.process_chunked()` with combine closure
5. `process_chunked()` iterates channel-by-channel, chunk-by-chunk; rayon parallelizes rows
6. Per-pixel: gather values from all frames, apply normalization, call combine function
7. Combine function calls `rejection.combine_mean()` which rejects then computes (weighted) mean

## Rejection Algorithm Analysis

### Sigma Clipping -- CORRECT

Implementation matches PixInsight and is more robust than Siril's default and DSS:
- Center: median (matches PixInsight; Siril default uses mean, less robust; DSS uses mean)
- Spread: MAD * 1.4826 (matches PixInsight; DSS and Siril default use stddev, inflated by outliers)
- Iterative with configurable max iterations (default 3)
- Asymmetric sigma_low/sigma_high thresholds (matches PixInsight convention)
- Min 2 values preserved (no rejection on tiny stacks)
- Early-exit optimization: `no_outliers_possible()` uses Welford's single-pass trimmed mean+stddev
  to skip expensive median+MAD when no outlier can exceed the threshold. Applied for N >= 10.
  Correctness validated by end-to-end tests confirming it does not suppress valid rejection.

Correctness: Using median + MAD is strictly superior to mean + stddev for outlier rejection
because the statistics being used to detect outliers are not themselves corrupted by the outliers.
DSS's Kappa-Sigma uses mean + stddev which causes under-rejection when bright outliers inflate
the standard deviation. IRAF's `sigclip` similarly uses mean + stddev by default.

vs IRAF `avsigclip`: IRAF's averaged sigma clipping computes sigma from images that have
already had their high/low values excluded, then applies this "average sigma" to the full stack.
Our approach is more robust because median+MAD never includes outliers in the estimator.

### Winsorized Sigma Clipping -- CORRECT

Two-phase algorithm matching PixInsight:
- Phase 1 (robust estimation): Iteratively Winsorize with Huber's c=1.5, compute stddev
  (not MAD), apply 1.134 bias correction factor, converge when `|delta_sigma/sigma| < 0.0005`,
  max 50 iterations. Median recomputed on Winsorized data each iteration.
- Phase 2 (rejection): Standard sigma clipping using the robust (center, sigma) from phase 1.

The 1.134 correction factor compensates for the bias introduced by Winsorization truncating the
tails of the distribution. Without it, sigma is underestimated, leading to over-rejection.

Key correctness detail: Phase 1 uses stddev (not MAD) because the data has been Winsorized --
outliers have been clamped to boundary values, making stddev a valid estimator on the modified
distribution. MAD would be unnecessarily conservative on already-Winsorized data.

Siril forum discussion confirms this matches their implementation: Huber c=1.5 for boundaries,
1.134 correction, convergence check on sigma change.

### Linear Fit Clipping -- CORRECT

Matches PixInsight/Siril:
- First pass: robust median + MAD sigma clipping (initial outlier removal)
- Subsequent passes: sort survivors with index co-array, fit `y = a + b * sorted_index`
  via least squares, compute sigma as mean absolute deviation of residuals from fit,
  reject each pixel against its own fitted value `a + b*i`
- Per-pixel rejection against fitted value (not single center) is critical for correctness

The linear fit models the expected distribution of pixel values across frames after sorting.
Outliers deviate from this linear trend. Using mean absolute deviation (not stddev or MAD)
for sigma is intentional -- it provides a balanced estimate for the residuals from a linear fit
that is less sensitive to the remaining outliers than stddev but not as conservative as MAD.

Min 3 values preserved (linear fit needs at least 3 points).

### Percentile Clipping -- DIFFERENT SEMANTICS (Acceptable)

Our implementation: rank-based (clip N% from each end of sorted values).
Siril: distance-based from median (`reject if |pixel - median| > median * factor`).
PixInsight: also distance-based from median.
IRAF `pclip`: sigma-based at percentile-derived sigma, different again.

Both approaches are valid. Rank-based is simpler, more predictable, and does not depend on
the actual distribution shape. Distance-based adapts to the distribution (no rejection if all
values agree closely). For typical astrophotography use (small stacks, 3-10 frames), rank-based
is appropriate. The main practical difference: our approach always rejects a fixed count;
distance-based may reject zero or many depending on data spread.

Edge case: extreme percentiles (e.g. 49% + 49%) are handled via `surviving_range()` which
guarantees at least 1 element survives.

### GESD (Generalized ESD) -- CORRECT WITH INTENTIONAL DEVIATION

Two-phase approach matching NIST description:
- Phase 1: Iteratively find most extreme value, compute test statistic `R_i = max |x - center| / sigma`,
  tentatively remove it. Record all R_i values.
- Phase 2: Backward scan comparing R_i against critical values lambda_i to determine actual outlier count.
  `lambda_i = (n-i) * t_p,n-i-1 / sqrt[(n-i-1+t^2)(n-i+1)]` where `p = 1 - alpha/(2*(n-i+1))`.

Intentional deviations from NIST:
- Uses median + MAD instead of mean + stddev. NIST specifies mean + stddev, but in astrophotography
  context the very outliers being detected corrupt the mean and inflate stddev. PixInsight uses
  trimmed mean + trimmed stddev as a compromise. Our median + MAD is more robust. For clean data
  median approximates mean and MAD*1.4826 approximates stddev, so critical values remain valid.
- Asymmetric relaxation via `low_relaxation` factor (default 1.5, matching PixInsight). Dark pixels
  below median use `sigma * low_relaxation` as effective sigma, reducing their test statistic and
  making them harder to reject. This is astrophotography-specific: dark outliers (cosmic ray gaps,
  cold pixels) are less problematic than bright outliers (satellites, hot pixels).
- Uses Abramowitz & Stegun rational approximation for inverse normal CDF instead of full
  t-distribution. Accurate for n >= 15; NIST notes critical values are "very accurate for n >= 25."
  Maximum absolute error ~4.5e-4.

NIST recommends n >= 25 for reliable results. For small stacks (< 15 frames), GESD may under-reject.
This matches PixInsight's recommendation: use GESD only for large stacks (50+ frames).

Default `max_outliers` is 25% of data size. Min 3 values preserved.

### No Rejection -- TRIVIALLY CORRECT

Returns all values unchanged. Used for bias frames or when rejection is not needed.

## Normalization Review

### Current Implementation

- **None**: No adjustment. Correct for bias and dark frames.
- **Global**: `gain = ref_mad / frame_mad`, `offset = ref_median - frame_median * gain`.
  Matches Siril's "additive with scaling" mode. Corrects both brightness offset and scale
  (contrast) differences between frames. Best for light frames with varying sky brightness
  and transparency.
- **Multiplicative**: `gain = ref_median / frame_median`, `offset = 0`.
  Pure scaling by median ratio. Best for flat frames where exposure time varies but the
  illumination pattern is consistent. No additive offset prevents introducing bias.

### Formulas Correctness

Global normalization formula is correct. For frame `f` relative to reference `r`:
- `gain_f = MAD_r / MAD_f` -- scales dispersion to match reference
- `offset_f = median_r - median_f * gain_f` -- shifts level after scaling

This ensures `normalized_median = median_f * gain_f + offset_f = median_r` (levels match)
and `normalized_MAD = MAD_f * gain_f = MAD_r` (dispersions match).

Multiplicative formula `gain_f = median_r / median_f` ensures `normalized_median = median_r`.
No scale matching, which is correct for flats (relative illumination pattern matters, not
absolute noise level).

Edge case: `frame_mad <= f32::EPSILON` or `frame_median <= f32::EPSILON` results in gain = 1.0
(no change). This prevents division by zero and is reasonable since a zero-MAD frame has no
meaningful dispersion to match, and a zero-median flat frame is degenerate.

### Reference Frame Selection

Auto-selected by lowest average MAD across channels. The lowest-noise frame provides the most
stable normalization target. Ties broken by first frame (deterministic).

This is a reasonable heuristic. PixInsight uses a composite quality metric (noise, FWHM,
eccentricity). Siril uses the "best" image by a configurable criterion. Our MAD-based selection
captures the most important dimension (noise) without requiring star detection or PSF fitting.

### Statistics Estimators

Currently uses median and MAD. Siril defaults to IKSS (Iterative Kappa-Sigma Statistics):
1. Compute median and MAD
2. Discard pixels > 6*MAD from median
3. Recompute median (location) and sqrt(BWMV) (scale) on clipped dataset

BWMV (Biweight Midvariance) is a robust weighted variance where weights decrease with distance
from median, giving less influence to borderline outliers. sqrt(BWMV) is a more efficient
scale estimator than MAD for approximately Gaussian data.

Impact assessment: For typical astrophotography data (mostly Gaussian background + sparse bright
objects), the difference between MAD and IKSS/BWMV is marginal. The 6*MAD clipping step helps
when bright nebulosity or star fields skew statistics, but this primarily affects normalization
quality, not rejection quality. Our MAD-based approach matches Siril's "fast mode" option.

### Normalization Mode Coverage

| Mode | Ours | Siril | PixInsight | APP |
|------|------|-------|------------|-----|
| None | Yes | Yes | Yes | Yes |
| Additive only | No | Yes | Yes | Yes |
| Multiplicative only | Yes | Yes | Yes | Yes |
| Additive + Scaling | Yes (Global) | Yes (default for lights) | Yes | Yes |
| Multiplicative + Scaling | No | Yes | Yes | No |
| Local normalization | No | No | Yes (separate process) | Yes (LNC) |

## Industry Comparison

### vs PixInsight ImageIntegration

| Feature | This Implementation | PixInsight |
|---------|-------------------|------------|
| Sigma clip center | Median | Median |
| Sigma clip spread | MAD * 1.4826 | MAD * 1.4826 |
| Asymmetric sigma | Yes (sigma_low/high) | Yes (sigma low/high) |
| Winsorized | Huber c=1.5, convergence, 1.134 correction, then sigma clip | Huber c=1.5, convergence, 1.134 correction, then sigma clip |
| Linear fit | Per-pixel comparison against fitted value | Per-pixel comparison against fitted value |
| Linear fit sigma | Mean absolute deviation from fit | Mean absolute deviation from fit |
| GESD statistics | Median + MAD (more robust) | Trimmed mean + trimmed stddev |
| GESD relaxation | Yes (default 1.5 for low pixels) | Yes (default 1.5 for low pixels) |
| Normalization | 3 modes (None/Global/Mult) | 5 modes + Local normalization |
| Rejection normalization | Same as combination | Separate from combination normalization |
| Weighting | Equal, Noise (1/σ²), Manual | Noise eval (MRS), PSF signal, PSF SNR |
| Reference frame | Auto-select by lowest noise (MAD) | Auto-select by quality metric |
| Combine methods | Mean, Median | Mean, Median, Min, Max |
| Rejection maps | Not generated | Low/High rejection maps + slope map |
| Large-scale rejection | Not implemented | Layers + growth for satellite trails |
| Variance propagation | Not implemented | Full noise model tracking |
| Drizzle | Separate module (4 kernels, projective) | Full drizzle integration |
| Memory model | 75% RAM or mmap disk cache | On-demand FITS row reading |

### vs Siril

| Feature | This Implementation | Siril |
|---------|-------------------|-------|
| Scale estimator | MAD | IKSS (default), MAD (fast mode), sqrt(BWMV) |
| Location estimator | Median | IKSS (default), Median (fast mode) |
| Normalization modes | 3 (None/Global/Mult) | 5 (None/Add/Mult/Add+Scale/Mult+Scale) |
| Winsorized correction | Yes, 1.134 * stddev | Yes, 1.134 * stddev |
| Linear fit sigma | Mean absolute deviation from fit | Mean absolute deviation, per-pixel |
| Weighting | Equal, Noise (1/σ²), Manual | Automatic: noise, FWHM, star count, integration time |
| Rejection maps | No | Yes (low/high, mergeable) |
| Percentile clipping | Rank-based | Distance-based from median |
| Block processing | Per-channel sequential | Channel-in-block (parallel channels) |
| Memory threshold | 75% of available | 90% of available |
| Max open files | No limit (mmap) | OS limit (2048 on Windows) |
| Normalization cache | No | Cached in sequence file |

### vs DeepSkyStacker

| Feature | This Implementation | DSS |
|---------|-------------------|-----|
| Sigma clip center | Median | Mean |
| Sigma clip spread | MAD * 1.4826 | Standard deviation |
| Rejection algorithms | 6 (sigma, winsorized, linear fit, percentile, GESD, none) | 4 (kappa-sigma, median kappa-sigma, auto adaptive, entropy) |
| Winsorized | Full two-phase | "Median Kappa-Sigma" replaces rejected with median |
| Calibration defaults | Frame-type presets | Median for bias/dark/flat masters |
| Weighting | Equal, Noise (1/σ²), Manual | None (equal weight) |
| Normalization | 3 modes | Basic (match average luminosity) |

Our sigma clip is strictly more robust than DSS's kappa-sigma (median+MAD vs mean+stddev).
DSS's "Auto Adaptive Weighted Average" iteratively weights pixels by deviation from mean --
a unique approach not found in PixInsight or Siril but effective for mixed-exposure stacks.

### vs AstroPixelProcessor (APP)

| Feature | This Implementation | APP 2.0 |
|---------|-------------------|---------|
| Normalization | 3 modes | Advanced + Local Normalization Correction (LNC) |
| LNC | Not implemented | 8th degree polynomial local correction |
| Rejection | 6 algorithms | Sigma clip, winsorized sigma clip |
| Multi-band blending | Not implemented | MBB for mosaic seams |
| Weighting | Equal, Noise (1/σ²), Manual | Automatic quality-based |
| Mosaic support | Not applicable | Full mosaic integration |

APP's key differentiator is Local Normalization Correction (LNC) which corrects per-region
gradient differences before integration, producing dramatically better rejection in gradient-heavy
data. This is the most impactful missing feature for real-world imaging conditions.

### vs IRAF imcombine

| Feature | This Implementation | IRAF imcombine |
|---------|-------------------|----------------|
| Rejection | 6 methods | 7 methods (none, minmax, ccdclip, crreject, sigclip, avsigclip, pclip) |
| CCD noise model | Not used | `(rdnoise/gain)^2 + DN/gain + (snoise*DN)^2` for ccdclip/crreject |
| Variance propagation | No | Yes (full noise model) |
| CR-only rejection | Asymmetric sigma | Dedicated `crreject` (reject positive only) |
| Scaling | 3 normalization modes | mode, median, mean, exposure scaling |
| Zero/statsec | Not implemented | Per-image zero offset and statistics section |
| Output options | Combined image only | Combined + sigma + n-rejected images |

IRAF's `ccdclip` and `crreject` use the CCD noise equation (read noise + shot noise + scintillation)
to compute expected variance at each pixel, giving a physically motivated rejection threshold.
This is more principled than statistical rejection for single-exposure cosmic ray removal.

## Missing Features

### P1: No Variance Propagation -- MEDIUM SEVERITY

Neither the combined pixel value nor a per-pixel noise estimate is produced. PixInsight
propagates variance through the full pipeline, producing noise estimates for the integrated
image. IRAF's imcombine can output sigma images. Without variance propagation:
- Downstream processes (photometry, source detection) cannot use noise-weighted operations
- No objective quality metric for the stacked result
- Cannot implement inverse-variance weighting in a second pass

Impact: Significant for scientific use cases; less important for visual astrophotography.

### P2: Missing Rejection Maps Output -- MEDIUM SEVERITY

Both PixInsight and Siril generate per-pixel rejection count maps (low/high). PixInsight also
generates a slope map for linear fit. Diagnostic only -- does not affect stacking results.
Most requested feature for parameter tuning. Fix: track rejected counts during `combine_mean`,
return alongside combined value.

### ~~P2: Missing Automatic Weighting~~ -- IMPLEMENTED

Noise-based inverse variance weighting `w = 1/sigma_bg^2` implemented via `Weighting::Noise`.
Computes weights from per-channel MAD (already computed for normalization — zero extra I/O).
`StackConfig::light()` preset uses `Weighting::Noise` by default.

Still missing vs industry: FWHM-based weighting, PSF signal/SNR weighting (PixInsight),
star count weighting (Siril). These require star detection per frame.

### P2: Missing Separate Rejection vs Combination Normalization -- LOW SEVERITY

PixInsight provides two independent normalization controls. In practice, using the same
normalization for both works for the vast majority of workflows. Separate controls only matter
for preserving absolute flux while using normalized rejection -- a niche advanced use case.

### P3: Missing Additive-Only Normalization -- LOW SEVERITY

Formula: `offset = ref_median - frame_median`, `gain = 1.0`. Useful for calibration frames
with varying pedestal but consistent gain. Trivial to add to `Normalization` enum.

### P3: Missing Min/Max/Sum Combine Methods -- LOW SEVERITY

- Maximum: star-trail images, hot pixel identification
- Minimum: dark current floor, cold pixel identification
- Sum: total signal accumulation (trivial `CombineMethod::Sum` variant)
- DSS offers "Auto Adaptive Weighted Average" and "Entropy Weighted Average" -- niche methods

### P3: Missing Sigma Clipping Convergence Mode -- LOW SEVERITY

Astropy supports `maxiters=None` (iterate until no values rejected). Siril iterates until
convergence. Our implementation only supports fixed iteration count. For most astrophotography
stacks (10-50 frames), 3 iterations is sufficient.

### P3: Missing IKSS/BWMV Statistics Estimators -- LOW SEVERITY

Siril's default normalization uses IKSS (clip 6*MAD, recompute with BWMV). Our median+MAD
matches Siril's fast fallback mode. Impact is marginal for typical data but could improve
normalization quality when bright nebulosity or dense star fields are present.

### P3: Missing Large-Scale Rejection -- MEDIUM SEVERITY (niche)

PixInsight offers "large-scale rejection" for satellite trails and aircraft: wavelet
decomposition into layers, then growth/dilation to reject connected bright structures.
Standard pixel-by-pixel rejection misses faint satellite trails that are only slightly above
the noise at each pixel but clearly visible as coherent structures. This is a significant
feature gap for light-polluted imaging sites with many satellite passes.

APP also handles this via its Multi-Band Blending approach which removes stack artifacts.

### P3: Missing CCD Noise Model Rejection -- LOW SEVERITY

IRAF's `ccdclip`/`crreject` use gain + readnoise + scintillation to compute per-pixel
expected variance. More principled than statistical rejection for cosmic ray detection,
especially with few frames (works with as few as 2). However, requires calibration metadata
that may not always be available in amateur workflows.

## Performance Analysis

### Algorithmic Complexity

Per pixel, per iteration:
- **Sigma clip**: O(N) for median (quickselect) + O(N) for MAD + O(N) partition = O(N) amortized
- **Winsorized**: O(N * WINSORIZE_MAX_ITER) for phase 1 + O(N) for phase 2
- **Linear fit**: O(N log N) sort + O(N) fit + O(N) rejection per iteration
- **Percentile**: O(N log N) sort (one-shot, no iterations)
- **GESD**: O(N * max_outliers) -- each step computes median + MAD + scan
- **No rejection**: O(N) for mean

### Optimizations Present

1. **Early exit** (`no_outliers_possible`): Cheap single-pass trimmed mean+stddev check
   skips expensive median+MAD for clean pixels. Applied when N >= 10. For typical astronomical
   images where ~95% of pixels are background, this saves two quickselect operations per pixel
   on the vast majority of the image.

2. **Adaptive sorting**: Insertion sort for N <= 64 (optimal for typical 10-50 frame stacks),
   introsort via `sort_unstable_by` for N > 64. Avoids O(N^2) worst case.

3. **Per-thread scratch buffers**: `ScratchBuffers` allocated once per rayon thread via
   `for_each_init`, reused across all pixels. Zero per-pixel allocation.

4. **Chunked processing**: In-memory mode processes all rows in one chunk. Disk-backed mode
   uses adaptive chunk sizing based on available memory: `chunk_rows = usable_memory /
   (width * sizeof(f32) * frame_count)`. Minimum 64 rows to avoid excessive I/O overhead.

5. **Memory-mapped I/O**: Disk-backed mode uses `mmap` with `MADV_SEQUENTIAL` for kernel
   read-ahead. bytemuck zero-copy f32 access from page-aligned mappings.

6. **Parallel loading**: Limited to 3 concurrent threads to balance throughput vs I/O pressure.

### Potential Optimizations Not Yet Implemented

1. **SIMD rejection**: The per-pixel gather loop (reading one value from each frame) is
   inherently scatter-gather, making SIMD difficult. However, the rejection inner loop
   (compare against threshold, compact survivors) could benefit from AVX2 masked operations
   for large N. Estimated impact: ~20-30% for N > 64.

2. **Compensated summation in weighted_mean_indexed**: The `weighted_mean_indexed()` function
   in rejection.rs uses naive summation while `math::weighted_mean_f32()` uses Neumaier
   compensated summation. For typical stacking (N < 100, values in similar range), the
   difference is negligible, but it is an inconsistency.

3. **Cross-channel parallelism**: Currently processes channels sequentially. Siril processes
   channels within blocks for more parallelism. Impact: small for in-memory mode (one chunk),
   potentially meaningful for disk-backed mode with small chunks.

4. **Configurable I/O parallelism**: 3 concurrent loading threads is conservative for NVMe SSD.
   Auto-detection of storage type or user configuration could improve loading throughput.

5. **Normalization statistics caching**: `compute_channel_stats()` is sequential and computes
   statistics for all frames one by one. Could be parallelized across frames.

## Recommendations

### Priority Order

1. **Add rejection maps** (P2) -- per-pixel high/low rejection counts for diagnostics.
   Most requested feature for parameter tuning.
2. ~~**Add noise-based auto weighting**~~ -- **DONE** (`Weighting::Noise`, `w = 1/sigma_bg^2`)
3. **Add variance propagation** (P1 for science, P3 for visual) -- track per-pixel noise
   through the pipeline. Enables downstream noise-aware processing.
4. **Add additive-only normalization** (P3) -- trivial.
5. **Add Min/Max/Sum combine methods** (P3) -- trivial.
6. **Add IKSS/BWMV statistics** (P3) -- moderate effort, marginal improvement.
7. **Add sigma clip convergence mode** (P3) -- iterate until no rejection.
8. **Add large-scale rejection** (P3) -- significant effort, high value for satellite sites.
9. **Consider compensated summation in weighted_mean_indexed** (P3) -- minor consistency fix.

### What We Do Well

- **MAD-based sigma**: More robust than Siril's default clipped stddev and DSS's mean+stddev
- **ScratchBuffers per rayon thread**: No per-pixel allocation (PixInsight allocates per pixel)
- **Compile-time safety**: `CombineMethod::Mean(Rejection)` makes invalid combinations
  unrepresentable (e.g., median + rejection)
- **Adaptive storage**: Auto in-memory vs disk-backed (mmap) based on available RAM
- **Index tracking**: Maintains frame-to-weight mapping through rejection reordering via
  `weighted_mean_indexed()` with index co-array
- **Asymmetric sigma clipping**: Proper separate low/high thresholds
- **GESD two-phase**: Correct forward removal + backward scan matching NIST description
- **Normalization formulas**: Global matches Siril's "additive with scaling"
- **Winsorized correctness**: Full two-phase with Huber c=1.5, 1.134 correction, convergence
- **Large-N sort**: Adaptive insertion sort (N<=64) / introsort (N>64) with index co-array
- **Thorough test coverage**: ~90+ tests including weight alignment, edge cases, cross-validation
- **Early exit optimization**: Cheap trimmed-stats check avoids median+MAD on clean pixels
- **Frame-type presets**: Sensible defaults for bias/dark/flat/light matching industry conventions

## Memory Management

- **In-memory mode**: When total image data < 75% of available RAM
- **Disk-backed mode**: Per-channel binary files with mmap; FNV-1a hash-based filenames
- **Chunked processing**: Rows in chunks sized to fit memory; `chunk_rows =
  usable_memory / (width * sizeof(f32) * frame_count)` (processes one channel at a time)
- **Parallel I/O**: Loading limited to 3 concurrent threads via `try_par_map_limited`.
  Conservative for HDD (prevents seek thrashing); suboptimal for NVMe SSD (could use 6-8)
- **Per-thread scratch**: `ScratchBuffers` allocated once per rayon thread via `for_each_init`
- **Cache cleanup**: `Drop` impl removes cache files unless `keep_cache` set
- **Cache validation**: `.meta` sidecar files store source mtime for staleness detection
- **bytemuck alignment**: mmap returns page-aligned addresses (4096-byte); f32 needs
  4-byte alignment. Always safe.
- **Memory budget**: 75% vs Siril's 90%. More conservative but safer across platforms.
  The 25% headroom absorbs `sysinfo::available_memory()` measurement fluctuations.

## Weighting

Three strategies via `Weighting` enum in `StackConfig`:
- **Equal** (default for bias/dark/flat): No weighting. All frames contribute equally.
- **Noise** (default for light): Automatic `w = 1/sigma_bg^2` where sigma is the average
  MAD-derived sigma across channels. Computed from channel stats already available from
  normalization — zero extra I/O. Weights normalized to sum=1.
- **Manual**: User-provided per-frame weights, normalized to sum=1.

Resolution happens in `resolve_weights()` in `stack.rs`:
- `Equal` → `None` (no weight array, falls through to unweighted mean)
- `Noise` → computes from `ChannelStats` (shared with normalization)
- `Manual` → normalizes user weights

Index tracking preserves correct weight-to-value mapping after rejection reordering.
`weighted_mean_indexed()` uses the index co-array to look up original frame weights
after rejection functions have reordered the values array.

Still missing vs industry:
- FWHM-based weighting (`w = 1/(sigma_bg^2 * FWHM^2)` for point sources)
- PSF-based weighting (PixInsight: PSF signal weight, PSF SNR)
- Star count weighting (Siril)
These require per-frame star detection.

## Normalization Details

- **None**: No adjustment (correct for bias/dark frames)
- **Global**: `gain = ref_mad / frame_mad`, `offset = ref_median - frame_median * gain`
  Matches Siril's "additive with scaling". Best for light frames.
- **Multiplicative**: `gain = ref_median / frame_median`, `offset = 0`
  Best for flat frames where exposure varies.
- Reference frame: auto-selected by lowest average MAD across channels
- Statistics: per-channel median and MAD via `compute_channel_stats()`
- Missing: separate rejection vs combination normalization (PixInsight feature)
- Missing: IKSS estimator (6*MAD clip, then recompute with median + sqrt(BWMV) -- Siril default)
- Missing: pure additive mode, multiplicative+scaling mode, local normalization

## Frame Type Handling

`FrameType` (Dark/Flat/Bias/Light) is used for logging and error messages only. It does NOT
affect algorithm behavior. Stacking parameters are controlled entirely by `StackConfig`.

Frame-type presets set appropriate defaults:
- `StackConfig::bias()`: Winsorized sigma=3.0, no normalization
- `StackConfig::dark()`: Winsorized sigma=3.0, no normalization
- `StackConfig::flat()`: Sigma clip sigma=2.5, multiplicative normalization
- `StackConfig::light()`: Sigma clip sigma=2.5, global normalization, noise weighting

This matches the industry approach. PixInsight and Siril also separate frame type labeling
from algorithm configuration.

## Edge Cases

- **Empty input**: Returns `Error::NoPaths`
- **Single frame**: All rejection algorithms return all values; normalization produces identity
- **Two frames**: Sigma clip, winsorized, and GESD skip rejection (min 2-3 preserved)
- **All identical values**: MAD=0, sigma=0 triggers early exit in all algorithms (no rejection)
- **All frames rejected**: `weighted_mean_indexed` returns 0.0 when all surviving weights
  are zero (epsilon guard, matches `math::weighted_mean_f32`).
- **Zero-weight frames**: `weighted_mean_indexed` guards against `weight_sum == 0` with
  epsilon check; returns 0.0 if all surviving weights are zero.
- **NaN values**: Source images are NaN-free (enforced by FITS float loader sanitization).
  `median_f32_fast` uses `partial_cmp` with `unwrap_or(Ordering::Equal)` — safe given this invariant.
- **Dimension mismatch**: Caught during loading; returns `Error::DimensionMismatch`
- **Weight count mismatch**: Caught by panic in `stack_with_progress`

## Test Coverage

- Unit tests for all six rejection algorithms (outlier removal, no-outlier preservation)
- Config construction and validation tests
- Asymmetric sigma clip behavior verification
- Index tracking through rejection for all reordering algorithms
- Weight-value alignment after rejection (sigma clip, winsorized, percentile, linear fit, GESD)
- Cross-validation: linear fit first pass == sigma clip single pass
- Normalization: identity for identical frames, offset/scale correction, RGB per-channel
- Reference frame selection: lowest MAD, RGB averaging, ties, single frame
- Dispatch: normalized vs unnormalized stacking comparison
- Cache: in-memory and disk-backed roundtrip, reuse detection, dimension mismatch, cleanup
- Cache: FNV-1a determinism pinned, source mtime validation
- Large-N tests: sort_with_indices (N=100, N=200), percentile (N=100), linear fit (N=100)
- GESD: relaxation correctness, boundary, symmetry, bright-only invariance
- Winsorized: robust estimate uses stddev not MAD, 1.134 correction applied, Huber c invariance
- Early exit: `no_outliers_possible` correctness with clean/outlier/moderate data
- Real data test (ignored): stacks registered lights from calibration directory
- Benchmarks: bias/dark/flat/light frames at 10/30/100 frame counts (1920x1280)

## References

- [PixInsight PCL -- IntegrationRejectionEngine.cpp](https://github.com/PixInsight/PCL/blob/master/src/modules/processes/ImageIntegration/IntegrationRejectionEngine.cpp)
- [PixInsight Image Weighting Algorithms](https://pixinsight.com/doc/docs/ImageWeighting/ImageWeighting.html)
- [PixInsight Forum -- Winsorized Sigma Clipping](https://pixinsight.com/forum/index.php?threads/image-integeration-winsorized-sigma-clipping-and-nuances.16768/)
- [PixInsight Forum -- Which Pixel Rejection Algorithm](https://pixinsight.com/forum/index.php?threads/which-pixel-rejection-algorithm.3094/)
- [PixInsight Local Normalization](https://chaoticnebula.com/pixinsight-local-normalization/)
- [PixInsight Image Integration Overview](https://chaoticnebula.com/pixinsight-image-integration/)
- [A detailed look into PixelRejection (DSLR Astrophotography)](https://dslr-astrophotography.com/detailed-pixel-rejection-methods/)
- [Siril Stacking Documentation (1.5.0)](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)
- [Siril Statistics Documentation (1.5.0)](https://siril.readthedocs.io/en/latest/Statistics.html)
- [Siril rejection_float.c (GitLab)](https://gitlab.com/free-astro/siril/-/blob/master/src/stacking/rejection_float.c)
- [Siril Normalization Algorithms (1.0)](https://free-astro.org/siril_doc-en/co/Average_Stacking_With_Rejection__2.html)
- [IRAF imcombine Documentation](https://iraf.readthedocs.io/en/doc-autoupdate/tasks/images/immatch/imcombine.html)
- [AstroPixelProcessor -- When to Use Which Outlier Rejection](https://www.astropixelprocessor.com/community/faq/when-to-use-which-outlier-rejection-filter/)
- [AstroPixelProcessor Unique Features](https://www.astropixelprocessor.com/with-unique-features/)
- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
- [NIST -- Generalized ESD Test](https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm)
- [DeepSkyStacker Technical Info](http://deepskystacker.free.fr/english/technical.htm)
- [DeepSkyStacker Theory](http://deepskystacker.free.fr/english/theory.htm)
- [GNU Astronomy Utilities -- Sigma Clipping](https://www.gnu.org/software/gnuastro/manual/html_node/Sigma-clipping.html)
- [Light Vortex Astronomy -- SNR with Kappa-Sigma Clipping](https://www.lightvortexastronomy.com/snr-increase-with-exposures-using-kappa-sigma-clipping-empirical-evidence.html)
- [Zackay & Ofek 2017 -- Optimal Coaddition](https://arxiv.org/abs/1512.06879)
- [Satellite Trail Removal in PixInsight](https://community.telescope.live/articles/pixinsight-tutorials/dealing-with-satellite-trails-in-pi-r117/)
- Bertin & Arnouts 1996 (SExtractor): A&AS 117, 393
