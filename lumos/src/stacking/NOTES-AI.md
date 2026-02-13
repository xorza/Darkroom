# stacking Module

## Architecture

Unified image stacking pipeline with six rejection algorithms, two combination methods,
three normalization modes, per-frame weighting, and automatic memory management.

### Files
- `mod.rs` - Public API, `FrameType` enum, re-exports
- `stack.rs` - `stack()`/`stack_with_progress()` entry points, normalization, dispatch
- `config.rs` - `StackConfig`, `CombineMethod`, `Normalization`, presets, validation
- `rejection.rs` - Six rejection algorithms with config structs, `Rejection` enum dispatch
- `cache.rs` - `ImageCache<I>` (in-memory or mmap), chunked parallel processing, `ScratchBuffers`
- `cache_config.rs` - `CacheConfig`, adaptive chunk sizing, system memory queries
- `error.rs` - `Error` enum (thiserror), I/O and dimension errors
- `progress.rs` - `ProgressCallback`, `StackingStage` (Loading/Processing)
- `tests/real_data.rs` - Integration test stacking registered lights (ignored, requires calibration dir)

### Data Flow
1. `stack_with_progress()` validates config, creates `ImageCache` from paths
2. `ImageCache::from_paths()` loads images; picks in-memory (<75% RAM) or disk-backed (mmap)
3. `compute_norm_params()` derives per-frame per-channel `NormParams { gain, offset }`
4. `dispatch_stacking()` calls `cache.process_chunked()` with combine closure
5. `process_chunked()` iterates channel-by-channel, chunk-by-chunk; rayon parallelizes rows
6. Per-pixel: gather values from all frames, apply normalization, call combine function
7. Combine function calls `rejection.combine_mean()` which rejects then computes (weighted) mean

## Rejection Algorithm Assessment

### Sigma Clipping -- CORRECT

Implementation matches PixInsight and is more robust than Siril's default:
- Center: median (matches PixInsight; Siril default uses mean, less robust)
- Spread: MAD * 1.4826 (matches PixInsight; DSS and Siril default use stddev, inflated by outliers)
- Iterative with configurable max iterations (default 3)
- Asymmetric sigma_low/sigma_high thresholds (matches PixInsight convention)
- Min 2 values preserved (no rejection on tiny stacks)

Correctness: Using median + MAD is strictly superior to mean + stddev for outlier rejection
because the statistics being used to detect outliers are not themselves corrupted by the outliers.
DSS's Kappa-Sigma uses mean + stddev which causes under-rejection when bright outliers inflate
the standard deviation.

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

### Percentile Clipping -- DIFFERENT SEMANTICS (Acceptable)

Our implementation: rank-based (clip N% from each end of sorted values).
Siril: distance-based from median (`reject if |pixel - median| > median * factor`).
PixInsight: also distance-based from median.

Both approaches are valid. Rank-based is simpler, more predictable, and does not depend on
the actual distribution shape. Distance-based adapts to the distribution (no rejection if all
values agree closely). For typical astrophotography use (small stacks, 3-10 frames), rank-based
is appropriate. The main practical difference: our approach always rejects a fixed count;
distance-based may reject zero or many depending on data spread.

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

NIST recommends n >= 25 for reliable results. For small stacks (< 15 frames), GESD may under-reject.
This matches PixInsight's recommendation: use GESD only for large stacks (50+ frames).

### Inverse Normal Approximation

Uses Abramowitz & Stegun (1964) rational approximation with coefficients c0=2.515517, c1=0.802853,
c2=0.010328, d1=1.432788, d2=0.189269, d3=0.001308. Maximum absolute error ~4.5e-4. This
approximation is used to convert probability p to quantile z, substituting for the t-distribution
CDF in GESD critical value computation. For large n (degrees of freedom), t-distribution converges
to normal, so this is acceptable for typical stack sizes. Tests verify z(0.95) within 0.05 of 1.645
and z(0.975) within 0.05 of 1.96.

## Normalization Assessment

### Current Implementation

- **None**: No adjustment. Correct for bias and dark frames.
- **Global**: `gain = ref_mad / frame_mad`, `offset = ref_median - frame_median * gain`.
  Matches Siril's "additive with scaling" mode. Corrects both brightness offset and scale
  (contrast) differences between frames. Best for light frames with varying sky brightness
  and transparency.
- **Multiplicative**: `gain = ref_median / frame_median`, `offset = 0`.
  Pure scaling by median ratio. Best for flat frames where exposure time varies but the
  illumination pattern is consistent. No additive offset prevents introducing bias.

Reference frame: auto-selected by lowest average MAD across channels. The lowest-noise frame
provides the most stable normalization target. Ties broken by first frame (deterministic).

### Formulas Correctness

Global normalization formula is correct. For frame `f` relative to reference `r`:
- `gain_f = MAD_r / MAD_f` -- scales dispersion to match reference
- `offset_f = median_r - median_f * gain_f` -- shifts level after scaling

This ensures `normalized_median = median_f * gain_f + offset_f = median_r` (levels match)
and `normalized_MAD = MAD_f * gain_f = MAD_r` (dispersions match).

Multiplicative formula `gain_f = median_r / median_f` ensures `normalized_median = median_r`.
No scale matching, which is correct for flats (relative illumination pattern matters, not
absolute noise level).

### Missing vs Industry

| Mode | Ours | Siril | PixInsight |
|------|------|-------|------------|
| None | Yes | Yes | Yes |
| Additive only | No | Yes (`offset = ref_med - frame_med, gain = 1`) | Yes |
| Multiplicative only | Yes | Yes | Yes |
| Additive + Scaling | Yes (Global) | Yes (default for lights) | Yes |
| Multiplicative + Scaling | No | Yes | Yes |
| Local normalization | No | No | Yes (separate process) |

Missing additive-only mode (P3): useful for darks/bias with varying pedestal. Trivial to add.
Missing multiplicative+scaling (P3): `gain = ref_MAD/frame_MAD`, `offset` via multiplication.
Local normalization (PixInsight-only): tile-based per-region normalization that dramatically
improves rejection for images with gradients. Significant complexity, evaluated and deferred.

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

## Cache System Assessment

### Architecture -- SOUND

The two-tier storage approach is well-designed:
- In-memory when total data < 75% of available RAM
- Disk-backed with memory-mapped per-channel binary files otherwise

Key design decisions that are correct:
- Per-channel files (not per-frame) enable efficient planar access during processing
- FNV-1a deterministic hashing for stable cache filenames across runs
- Source mtime validation via `.meta` sidecar files prevents stale cache use
- `madvise(MADV_SEQUENTIAL)` on Unix for kernel read-ahead
- `bytemuck::cast_slice` for zero-copy f32 access from mmap (page-aligned, always safe)
- `Drop` impl ensures cleanup unless `keep_cache` is set

### Memory Budget -- CONSERVATIVE

75% of available memory vs Siril's 90%. This is more conservative but safer across platforms.
The 25% headroom absorbs `sysinfo::available_memory()` measurement fluctuations and leaves
room for OS/application needs. Siril compensates for the higher threshold by reducing thread
count when memory is insufficient rather than shrinking blocks.

### Processing Pattern

Outer loop: channels (sequential). Inner loop: row chunks (parallel via rayon).
Siril processes channels within blocks (more cross-channel parallelism).

The sequential channel processing is simpler and works well when chunks are large (in-memory mode
processes all rows in one chunk). For disk-backed mode with small chunks, the lack of
cross-channel parallelism means threads may idle while one channel's chunk is being read.

### Chunk Sizing Formula

`chunk_rows = usable_memory / (width * sizeof(f32) * frame_count)`

Note: processes one channel at a time, so channel count is not in the denominator. This is
correct because `process_chunks_internal` only reads one channel's data for all frames at a time.

### Parallel I/O

Loading limited to 3 concurrent threads via `try_par_map_limited`. Conservative for HDD (good --
prevents seek thrashing) but suboptimal for NVMe SSD where 6-8 threads would saturate bandwidth.
Could be made configurable or auto-detected from storage type.

### vs PixInsight

PixInsight reads FITS rows on demand (no pre-caching). Our approach writes all images to cache
first, adding an extra full I/O pass. This is a tradeoff: higher initial cost but simpler
random-access patterns during stacking. For re-processing with different parameters, our
`keep_cache` option amortizes the initial cost.

## Weight Computation Assessment

### Current State

- Manual per-frame weights via `StackConfig::weights`
- Weights normalized to sum to 1.0 before use
- Index tracking preserves correct weight-to-value mapping after rejection reordering
- `weighted_mean_indexed()` uses the index co-array to look up original frame weights
  after rejection functions have reordered the values array

### Missing Automatic Weighting Schemes

**Noise-based (inverse variance)**: `w = 1/sigma_bg^2`. Theoretically optimal for maximizing
SNR in the combined image (maximum likelihood estimator). Requires reliable background noise
estimation. PixInsight uses MRS (Multiresolution Support) wavelet-based noise estimation on
the first 4 wavelet layers assuming Gaussian distribution. Siril uses the iterative k-sigma
background noise estimate from its statistics module.

**FWHM-based**: `w = 1/(sigma_bg^2 * FWHM^2)` for point source optimization. Penalizes
frames with poor seeing in addition to noise. PixInsight's SubframeSelector computes FWHM
via PSF fitting (elliptical Gaussian or Moffat).

**PSF Signal Weight (PixInsight)**: Combines total PSF flux (signal strength), mean PSF flux
(resolution/sharpness), noise estimate, and background level into a single quality metric.
Values typically 0.01-100 for deep-sky. This is the most sophisticated weighting scheme and
captures dataset-specific quality dimensions that simpler formulas miss.

**PSF SNR (PixInsight)**: Signal-to-noise from PSF photometry. Ratio of squared PSF flux to
squared noise estimates. Focuses on stellar signal quality rather than global image statistics.

**Siril weighting options**: Number of stars, weighted FWHM (FWHM weighted by star count),
noise, integration time.

### Integration Path

The `weights` field in `StackConfig` provides the integration point. Automatic weight computation
belongs in a separate quality-assessment module that would:
1. Analyze each frame (detect stars, measure FWHM, estimate background noise)
2. Compute quality metrics
3. Derive per-frame weights
4. Pass weights to the stacking pipeline

This separation is correct -- weighting is a pre-processing step, not part of the stacking
algorithm itself.

## Frame Type Handling

### Current Behavior

`FrameType` (Dark/Flat/Bias/Light) is used for logging and error messages only. It does NOT
affect algorithm behavior. Stacking parameters are controlled entirely by `StackConfig`.

Frame-type presets set appropriate defaults:
- `StackConfig::bias()`: Winsorized sigma=3.0, no normalization
- `StackConfig::dark()`: Winsorized sigma=3.0, no normalization
- `StackConfig::flat()`: Sigma clip sigma=2.5, multiplicative normalization
- `StackConfig::light()`: Sigma clip sigma=2.5, global normalization

### Assessment vs Industry

This matches the industry approach. PixInsight and Siril also separate frame type labeling
from algorithm configuration. The presets match standard recommendations:

| Frame | Rejection | Normalization | Rationale |
|-------|-----------|---------------|-----------|
| Bias | Winsorized 3.0 | None | No outliers expected; Winsorized preserves data. No normalization because bias level should be constant. |
| Dark | Winsorized 3.0 | None | Cosmic rays present; Winsorized handles small stacks well. No normalization because dark current at fixed temperature/exposure is constant. |
| Flat | Sigma clip 2.5 | Multiplicative | Dust motes are persistent (not outliers). Multiplicative corrects exposure variation while preserving the relative illumination pattern. |
| Light | Sigma clip 2.5 | Global | Satellites, cosmic rays, airplanes need rejection. Global normalization corrects sky brightness and transparency variations. |

DSS uses median for bias/dark/flat masters by default (simpler but loses information compared
to rejection + mean). Siril recommends similar parameters. PixInsight defaults are more complex
(different rejection per frame type, noise-based weighting).

## Issues vs Industry Standards

### ~~P1: Linear Fit -- Per-Pixel Rejection Against Fitted Value~~ -- FIXED
### ~~P1: Linear Fit -- Residual Sigma Uses Wrong Computation~~ -- FIXED
### ~~P1: Winsorized -- Missing 1.134 Correction Factor~~ -- FIXED
### ~~P1: Winsorized -- Wrong Architecture (Missing Two-Phase Approach)~~ -- FIXED
### ~~P2: GESD -- Missing Asymmetric Relaxation~~ -- FIXED
### ~~P2: GESD -- Statistics Mismatch with Critical Values~~ -- REJECTED (intentional)
### ~~P2: Reference Frame Always Frame 0~~ -- FIXED
### ~~P2: Large-Stack Sorting Performance (N > 100)~~ -- FIXED
### ~~P3: Cache -- DefaultHasher Non-Deterministic Across Runs~~ -- FIXED
### ~~P3: Cache -- Missing Source File Validation~~ -- FIXED
### ~~P3: Cache -- Missing madvise(MADV_SEQUENTIAL)~~ -- FIXED
### ~~P3: Missing Frame-Type-Specific Presets~~ -- FIXED

### P2: Missing Separate Rejection vs Combination Normalization -- POSTPONED

PixInsight provides two independent normalization controls. In practice, using the same
normalization for both works for the vast majority of workflows. Separate controls only matter
for preserving absolute flux while using normalized rejection -- a niche advanced use case.

### P2: Missing Rejection Maps Output -- POSTPONED

Both PixInsight and Siril generate per-pixel rejection count maps (low/high). PixInsight also
generates a slope map for linear fit. Diagnostic only -- does not affect stacking results.
Most requested feature for parameter tuning. Fix: track rejected counts during `combine_mean`,
return alongside combined value.

### P3: Missing Additive-Only Normalization

Formula: `offset = ref_median - frame_median`, `gain = 1.0`. Useful for calibration frames
with varying pedestal but consistent gain. Trivial to add to `Normalization` enum.

### P3: Missing Min/Max/Sum Combine Methods

- Maximum: star-trail images, hot pixel identification
- Minimum: dark current floor, cold pixel identification
- Sum: total signal accumulation (trivial `CombineMethod::Sum` variant)
- DSS offers "Auto Adaptive Weighted Average" and "Entropy Weighted Average" -- niche methods

### P3: Sigma Clipping -- Missing Convergence Mode

Astropy supports `maxiters=None` (iterate until no values rejected). Siril iterates until
convergence. Our implementation only supports fixed iteration count. For most astrophotography
stacks (10-50 frames), 3 iterations is sufficient.

### P3: Percentile Clipping -- Different Semantics from Industry

Rank-based vs distance-based. See rejection assessment section above.

### P3: Missing IKSS/BWMV Statistics Estimators

Siril's default normalization uses IKSS (clip 6*MAD, recompute with BWMV). Our median+MAD
matches Siril's fast fallback mode. Impact is marginal for typical data but could improve
normalization quality when bright nebulosity or dense star fields are present.

### P3: Missing Drizzle Integration

Drizzle (Variable-Pixel Linear Reconstruction) enables sub-pixel resolution recovery by mapping
input pixels onto a finer output grid accounting for sub-pixel shifts between frames. Developed
for Hubble Deep Field, now standard in PixInsight, DSS, and Astro Pixel Processor.

Requirements: 30+ frames, slightly undersampled data (stars < 2-3 pixels), known sub-pixel
registration offsets. Dramatically increases memory and processing time (650GB reported for
large Bayer drizzle). Output grid typically 1.5x-3x input resolution.

Integration with stacking: drizzle replaces the standard pixel-gather step. Instead of
gathering the same pixel from all frames, each input pixel contributes to a neighborhood of
output pixels weighted by the overlap area. Rejection still applies but operates on the
drizzle-accumulated values. This is architecturally separate from the current stacking pipeline
and would require its own processing path.

### P3: Missing Large-Scale Rejection

PixInsight offers "large-scale rejection" for satellite trails and aircraft: wavelet
decomposition into layers, then growth/dilation to reject connected bright structures.
Standard pixel-by-pixel rejection misses faint satellite trails that are only slightly above
the noise at each pixel but clearly visible as coherent structures. This is a significant
feature gap for light-polluted imaging sites with many satellite passes.

## Comparison with Industry Standards

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
| Weighting | Manual per-frame weights | Noise eval (MRS), PSF signal, PSF SNR |
| Reference frame | Auto-select by lowest noise (MAD) | Auto-select by quality metric |
| Combine methods | Mean, Median | Mean, Median, Min, Max |
| Rejection maps | Not generated | Low/High rejection maps + slope map |
| Large-scale rejection | Not implemented | Layers + growth for satellite trails |
| Drizzle | Not implemented | Full drizzle integration |
| Memory model | 75% RAM or mmap disk cache | On-demand FITS row reading |

### vs Siril

| Feature | This Implementation | Siril |
|---------|-------------------|-------|
| Scale estimator | MAD | IKSS (default), MAD (fast mode), sqrt(BWMV) |
| Location estimator | Median | IKSS (default), Median (fast mode) |
| Normalization modes | 3 (None/Global/Mult) | 5 (None/Add/Mult/Add+Scale/Mult+Scale) |
| Winsorized correction | Yes, 1.134 * stddev | Yes, 1.134 * stddev |
| Linear fit sigma | Mean absolute deviation from fit | Mean absolute deviation, per-pixel |
| Weighting | Manual | Automatic: noise, FWHM, star count, integration time |
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
| Weighting | Manual | None (equal weight) |
| Normalization | 3 modes | Basic (match average luminosity) |

Our sigma clip is strictly more robust than DSS's kappa-sigma (median+MAD vs mean+stddev).
DSS's "Auto Adaptive Weighted Average" iteratively weights pixels by deviation from mean --
a unique approach not found in PixInsight or Siril but effective for mixed-exposure stacks.

## What We Do Well

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

## Weighting

- Manual per-frame weights via `StackConfig::weights`
- Weights normalized to sum to 1.0 before use
- Index tracking preserves correct weight-to-value mapping after rejection reordering
- Missing: automatic noise-based weighting (`w = 1/sigma_bg^2`, inverse variance --
  theoretically optimal for maximizing SNR)
- Missing: FWHM-based weighting (`w = 1/(sigma_bg^2 * FWHM^2)` for point sources)
- Missing: PSF-based weighting (PixInsight: PSF signal weight, PSF SNR)
- The `weights` field provides the integration point; automatic computation belongs
  in a separate quality-assessment module

## Normalization

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

## Open Items (Priority Order)

1. **Add rejection maps** (P2) -- per-pixel high/low rejection counts for diagnostics.
   Most requested feature for parameter tuning.
2. **Add noise-based auto weighting** (P2) -- `w = 1/sigma_bg^2`. Highest-impact
   automatic weighting scheme. Requires reliable background noise estimator.
3. **Add additive-only normalization** (P3) -- trivial.
4. **Add Min/Max/Sum combine methods** (P3) -- trivial.
5. **Add IKSS/BWMV statistics** (P3) -- moderate effort, marginal improvement.
6. **Add sigma clip convergence mode** (P3) -- iterate until no rejection.
7. **Add large-scale rejection** (P3) -- significant effort, high value for satellite sites.
8. **Add drizzle integration** (P3) -- significant effort, separate processing path.

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
- Real data test (ignored): stacks registered lights from calibration directory

## References

- [PixInsight PCL -- IntegrationRejectionEngine.cpp](https://github.com/PixInsight/PCL/blob/master/src/modules/processes/ImageIntegration/IntegrationRejectionEngine.cpp)
- [PixInsight Image Weighting Algorithms](https://pixinsight.com/doc/docs/ImageWeighting/ImageWeighting.html)
- [PixInsight Forum -- Winsorized Sigma Clipping](https://pixinsight.com/forum/index.php?threads/image-integration-question-about-winsorized-sigma-clipping.1558/)
- [PixInsight Forum -- Which Pixel Rejection Algorithm](https://pixinsight.com/forum/index.php?threads/which-pixel-rejection-algorithm.3094/)
- [PixInsight Local Normalization](https://chaoticnebula.com/pixinsight-local-normalization/)
- [Siril Stacking Documentation (1.5.0)](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)
- [Siril Statistics Documentation (1.5.0)](https://siril.readthedocs.io/en/latest/Statistics.html)
- [Siril rejection_float.c (GitLab)](https://gitlab.com/free-astro/siril/-/blob/master/src/stacking/rejection_float.c)
- [Siril Normalization Algorithms (1.0)](https://free-astro.org/siril_doc-en/co/Average_Stacking_With_Rejection__2.html)
- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
- [NIST -- Generalized ESD Test](https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm)
- [DeepSkyStacker Technical Info](http://deepskystacker.free.fr/english/technical.htm)
- [DeepSkyStacker Theory](http://deepskystacker.free.fr/english/theory.htm)
- [Zackay & Ofek 2017 -- Optimal Coaddition](https://arxiv.org/abs/1512.06879)
- [Drizzle (Wikipedia)](https://en.wikipedia.org/wiki/Drizzle_(image_processing))
- [DSLR Astrophotography -- Pixel Rejection Methods](https://dslr-astrophotography.com/detailed-pixel-rejection-methods/)
- Bertin & Arnouts 1996 (SExtractor): A&AS 117, 393
