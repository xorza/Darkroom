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

### Data Flow
1. `stack_with_progress()` validates config, creates `ImageCache` from paths
2. `ImageCache::from_paths()` loads images; picks in-memory (<75% RAM) or disk-backed (mmap)
3. `compute_norm_params()` derives per-frame per-channel `NormParams { gain, offset }`
4. `dispatch_stacking()` calls `cache.process_chunked()` with combine closure
5. `process_chunked()` iterates channel-by-channel, chunk-by-chunk; rayon parallelizes rows
6. Per-pixel: gather values from all frames, apply normalization, call combine function
7. Combine function calls `rejection.combine_mean()` which rejects then computes (weighted) mean

## Issues vs Industry Standards

### ~~P1: Linear Fit -- Per-Pixel Rejection Against Fitted Value~~ — FIXED
### ~~P1: Linear Fit -- Residual Sigma Uses Wrong Computation~~ — FIXED

Rewritten to match PixInsight/Siril:
- Each pixel now compared against its own fitted value `a + b * i` (was single midpoint center)
- Sigma now uses mean absolute deviation: `(1/N) * sum |values[i] - (a + b*i)|` (was MAD of centered residuals)
- First pass still uses robust median + MAD for initial outlier removal

### ~~P1: Winsorized -- Missing 1.134 Correction Factor~~ — FIXED
### ~~P1: Winsorized -- Wrong Architecture (Missing Two-Phase Approach)~~ — FIXED

Rewritten to match PixInsight/Siril two-phase algorithm:
- Phase 1: Iteratively Winsorize with Huber's c=1.5, stddev (not MAD),
  1.134 bias correction, convergence criterion `|delta_sigma/sigma| < 0.0005`
- Phase 2: Standard sigma clipping rejection using the robust estimates
- Asymmetric sigma_low/sigma_high support added
- Now properly rejects outliers (via `reject()`) instead of averaging Winsorized values

### P2: GESD -- Missing Asymmetric Relaxation

- **File**: rejection.rs, `GesdConfig::reject`
- PixInsight multiplies sigma by a relaxation factor (default >= 1.5) for pixels
  **below** the trimmed mean, making the test more tolerant of dark outliers.
- Dark pixels (noise floor) should be rejected less aggressively than bright outliers
  (cosmic rays, satellites). Without relaxation, GESD over-rejects dark pixels.
- **Fix**: Add `low_relaxation: f32` field (default 1.5) to GesdConfig.

### P2: GESD -- Statistics Mismatch with Critical Values

- **File**: rejection.rs, `GesdConfig::reject`
- GESD critical values are derived assuming normal-distribution statistics (mean + stddev).
- Our median + MAD estimator is theoretically incompatible with these critical values.
- PixInsight uses **trimmed mean + trimmed stddev** (trimming fraction = max_outliers).
- Siril uses raw **mean + stddev** (classic Grubbs test).
- Our robust estimators may cause the test to behave differently from theoretical guarantees.
- **Fix**: Consider trimmed mean + stddev to match PixInsight, or document the deviation.

### P2: Missing Separate Rejection vs Combination Normalization

- **File**: stack.rs, config.rs
- PixInsight provides **two independent normalization controls**:
  1. Rejection normalization: applied before rejection to make outlier detection uniform
  2. Combination normalization: applied when computing final pixel values
- These can differ. Example: "Scale + Zero Offset" for rejection (match backgrounds),
  "No normalization" for output (preserve photometric values).
- Our implementation uses a single normalization for both.
- **Fix**: Add `StackConfig::rejection_normalization` field.

### P2: Reference Frame Always Frame 0

- **File**: stack.rs, `compute_norm_params`
- Always uses frame 0 (first loaded) as reference.
- A poor reference (high noise, gradient, cloud cover) degrades normalization and
  rejection quality for the entire stack.
- PixInsight auto-selects by SNRWeight; Siril auto-selects by noise level.
- **Fix**: After loading, compute per-frame `sigma_bg`, select lowest-noise frame.

### P2: Missing Rejection Maps Output

- Both PixInsight and Siril generate per-pixel rejection count maps (low/high).
- Critical for diagnosing whether rejection parameters are too aggressive or too lenient.
- PixInsight also generates a slope map for linear fit clipping.
- **Fix**: During `combine_mean`, track rejected counts. Return alongside combined value.

### P2: Large-Stack Sorting Performance (N > 100)

- **File**: rejection.rs, percentile and linear fit use insertion sort
- Insertion sort is O(N^2). For N=1000 (lucky imaging), ~500K comparisons per pixel.
  On a 6Kx4K image that's ~12 trillion comparisons.
- **Fix**: Switch to `sort_unstable` (introsort) for N > ~50 in percentile and linear fit.

### P3: Missing Additive-Only Normalization

- **File**: config.rs, `Normalization` enum
- Has None, Global (additive+scaling), Multiplicative. Missing pure additive (shift-only).
- Formula: `offset = ref_median - frame_median`, `gain = 1.0`.
- Useful for calibration frames (darks, bias) with varying pedestal but consistent gain.
- Siril and PixInsight both offer 5 normalization modes including pure additive.

### P3: Missing Min/Max Combine Methods

- Siril and PixInsight offer Minimum and Maximum combination.
- Maximum: star-trail images, hot pixel identification.
- Minimum: dark current floor, cold pixel identification.
- Trivial to implement.

### ~~P3: Cache -- DefaultHasher Non-Deterministic Across Runs~~ — FIXED
- Replaced `DefaultHasher` (random-seeded SipHash) with deterministic FNV-1a hasher.
  Cache filenames are now stable across process invocations, making `keep_cache` work.

### ~~P3: Cache -- Missing Source File Validation~~ — FIXED
- Added `.meta` sidecar files storing source mtime (8-byte LE u64).
  `load_and_cache_frame` writes meta after caching; `validate_source_meta` checks
  before reuse. Cleanup removes `.meta` files alongside channel caches.
- Test: `test_source_meta_validates_mtime`

### ~~P3: Cache -- Missing madvise(MADV_SEQUENTIAL)~~ — FIXED
- Added `mmap.advise(Advice::Sequential)` after mmap creation in `mmap_channel_file`.
  Gated with `#[cfg(unix)]`. Enables kernel read-ahead and early page release for
  the sequential row-by-row access pattern used during stacking.

### P3: Missing Frame-Type-Specific Presets

- `FrameType` enum affects only logging, not behavior.
- Industry-standard defaults differ by frame type:
  - Bias: Mean + Winsorized(3.0), Normalization::None
  - Dark: Mean + Winsorized(3.0), Normalization::None
  - Flat: Mean + SigmaClip(2.5), Normalization::Multiplicative
  - Light: Mean + SigmaClip(2.5), Normalization::Global
- **Fix**: Add preset constructors `StackConfig::bias()`, `dark()`, `flat()`, `light()`.

### P3: Sigma Clipping -- Missing Convergence Mode

- Astropy supports `maxiters=None` (iterate until no values rejected).
- Siril iterates until convergence (no rejection occurs).
- Our implementation only supports fixed iteration count.
- For most astrophotography stacks (10-50 frames), 3 iterations is sufficient.

### P3: Percentile Clipping -- Different Semantics from Industry

- Our implementation is **rank-based** (clip N% from each end of sorted values).
- Siril's "percentile clipping" is **distance-based from median**:
  `reject if |pixel - median| > median * percentile_factor`.
- Both are valid approaches. Rank-based is simpler and more predictable.
- Distance-based adapts to the actual distribution (no rejection if all values agree).

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
| GESD statistics | Median + MAD | Trimmed mean + trimmed stddev |
| GESD relaxation | Not implemented | Yes (default 1.5 for low pixels) |
| Normalization | 3 modes (None/Global/Mult) | 5 modes + Local normalization |
| Rejection normalization | Same as combination | Separate from combination normalization |
| Weighting | Manual per-frame weights | Noise eval (MRS), PSF signal, PSF SNR |
| Reference frame | Always frame 0 | Auto-select by quality metric |
| Combine methods | Mean, Median | Mean, Median, Min, Max |
| Rejection maps | Not generated | Low/High rejection maps + slope map |
| Large-scale rejection | Not implemented | Layers + growth for satellite trails |

### vs Siril

| Feature | This Implementation | Siril |
|---------|-------------------|-------|
| Scale estimator | MAD | IKSS (default), MAD (fast mode), sqrt(BWMV) |
| Location estimator | Median | IKSS (default), Median (fast mode) |
| Normalization modes | 3 (None/Global/Mult) | 5 (None/Add/Mult/Add+Scale/Mult+Scale) |
| Winsorized correction | Yes, 1.134 * stddev | Yes, 1.134 * stddev |
| Linear fit sigma | Mean absolute deviation from fit | Mean absolute deviation, per-pixel |
| Weighting | Manual | Automatic: noise, FWHM, star count |
| Rejection maps | No | Yes (low/high, mergeable) |
| Percentile clipping | Rank-based | Distance-based from median |
| Block processing | Per-channel sequential | Channel-in-block (parallel channels) |
| Memory threshold | 75% of available | 90% of available |

## What We Do Well

- **MAD-based sigma**: More robust than Siril's default clipped stddev and DSS's mean+stddev
- **ScratchBuffers per rayon thread**: No per-pixel allocation (PixInsight allocates per pixel)
- **Compile-time safety**: `CombineMethod::Mean(Rejection)` makes invalid combinations
  unrepresentable (e.g., median + rejection)
- **Adaptive storage**: Auto in-memory vs disk-backed (mmap) based on available RAM
- **Index tracking**: Maintains frame-to-weight mapping through rejection reordering
- **Asymmetric sigma clipping**: Proper separate low/high thresholds
- **GESD two-phase**: Correct forward removal + backward scan matching NIST description
- **Normalization formulas**: Global matches Siril's "additive with scaling"
- **Thorough test coverage**: ~90+ tests including weight alignment, edge cases, cross-validation

## Memory Management

- **In-memory mode**: When total image data < 75% of available RAM
- **Disk-backed mode**: Per-channel binary files with mmap; hash-based filenames
- **Chunked processing**: Rows in chunks sized to fit memory; chunk_rows =
  `usable_memory / (width * sizeof(f32) * frame_count)` (processes one channel at a time)
- **Parallel I/O**: Loading limited to 3 concurrent threads. Conservative for HDD;
  suboptimal for NVMe SSD (could use 6-8 for SSDs)
- **Per-thread scratch**: `ScratchBuffers` allocated once per rayon thread via `for_each_init`
- **Cache cleanup**: `Drop` impl removes cache files unless `keep_cache` set
- **bytemuck alignment**: mmap returns page-aligned addresses (4096-byte); f32 needs
  4-byte alignment. Always safe. Consider `try_cast_slice().expect()` for clarity.

### vs Siril Block Processing
- Siril's blocks include the channel dimension: different threads can work on different
  channels simultaneously. Our implementation processes channels sequentially in the
  outer loop, limiting cross-channel parallelism.
- Siril takes 90% of available memory (vs our 75%). If memory can't feed all threads,
  Siril reduces thread count rather than shrinking blocks.
- Siril's `refine_blocks_candidate()` balances block distribution across threads.

### vs PixInsight
- PixInsight reads rows directly from FITS on demand (no pre-caching phase).
  Our approach writes all images to cache files first, adding an extra I/O pass.
- PixInsight uses "buffer size" + "stack size" parameters for memory control.
  Users find that smaller buffer sizes sometimes perform better due to reduced
  memory pressure and better cache locality.

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
- Reference frame: always frame 0 (first loaded) -- should auto-select by quality
- Statistics: per-channel median and MAD via `compute_channel_stats()`
- Missing: separate rejection vs combination normalization (PixInsight feature)
- Missing: IKSS estimator (sigma-clip then recompute with BWMV -- Siril default)
- Missing: pure additive mode, local normalization

## Most Impactful Fixes (ordered by expected improvement)

1. ~~**Fix linear fit rejection** (P1)~~ — **FIXED**: per-pixel comparison against fitted
   value + mean absolute deviation sigma. Matches PixInsight/Siril.
2. ~~**Fix Winsorized architecture** (P1)~~ — **FIXED**: two-phase with Huber c=1.5,
   1.134 correction, convergence, stddev, asymmetric sigma_low/sigma_high.
3. **Add rejection maps** (P2) -- per-pixel high/low rejection counts for diagnostics.
   Most requested feature for parameter tuning.
4. **Add auto reference frame selection** (P2) -- select lowest-noise frame. Easy win.
5. **Add noise-based auto weighting** (P2) -- `w = 1/sigma_bg^2`. Highest-impact
   automatic weighting scheme. Requires reliable background noise estimator.
6. ~~**Fix cache hash determinism**~~ (P3) -- **FIXED**: FNV-1a hasher.
7. ~~**Add madvise(MADV_SEQUENTIAL)**~~ (P3) -- **FIXED**: `Advice::Sequential` on mmap.
8. **Add frame-type presets** (P3) -- `bias()`, `dark()`, `flat()`, `light()`.

## Test Coverage

- Unit tests for all six rejection algorithms (outlier removal, no-outlier preservation)
- Config construction and validation tests
- Asymmetric sigma clip behavior verification
- Index tracking through rejection for all reordering algorithms
- Weight-value alignment after rejection
- Cross-validation: linear fit first pass == sigma clip single pass
- Normalization: identity for identical frames, offset/scale correction, RGB
- Dispatch: normalized vs unnormalized stacking comparison
- Cache: in-memory and disk-backed roundtrip, reuse detection, dimension mismatch
- Real data test (ignored): stacks registered lights from calibration directory

## References

- [PixInsight PCL -- IntegrationRejectionEngine.cpp](https://github.com/PixInsight/PCL/blob/master/src/modules/processes/ImageIntegration/IntegrationRejectionEngine.cpp)
- [PixInsight Image Weighting Algorithms](https://pixinsight.com/doc/docs/ImageWeighting/ImageWeighting.html)
- [PixInsight Forum -- Winsorized Sigma Clipping](https://pixinsight.com/forum/index.php?threads/image-integration-question-about-winsorized-sigma-clipping.1558/)
- [Siril Stacking Documentation (1.5.0)](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)
- [Siril rejection_float.c (GitLab)](https://gitlab.com/free-astro/siril/-/blob/master/src/stacking/rejection_float.c)
- [Astropy sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
- [NIST -- Generalized ESD Test](https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm)
- [DeepSkyStacker Technical Info](http://deepskystacker.free.fr/english/technical.htm)
- [Zackay & Ofek 2017 -- Optimal Coaddition](https://arxiv.org/abs/1512.06879)
- Bertin & Arnouts 1996 (SExtractor): A&AS 117, 393
