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

## Rejection Algorithms

### Sigma Clipping (`SigmaClipConfig`)
- Iterative median + MAD-based (converted via 1.4826 factor)
- Supports asymmetric thresholds (sigma_low, sigma_high)
- Stops when no values rejected or <= 2 values remain
- Default: sigma=2.5, 3 iterations

### Winsorized Sigma Clipping (`WinsorizedClipConfig`)
- Replaces outliers with boundary values instead of removing them
- Uses median + MAD for center/sigma; clamps to [center - sigma*s, center + sigma*s]
- Does not modify original values (writes to working buffer)
- Default: sigma=2.5, 3 iterations

### Linear Fit Clipping (`LinearFitClipConfig`)
- First pass: median + MAD rejection (identical to sigma clip)
- Subsequent passes: sorts survivors, fits line y=a+bx through (sorted_index, value)
- New center from fit at median position; new sigma from MAD of residuals
- Default: sigma_low=3.0, sigma_high=3.0, 3 iterations

### Percentile Clipping (`PercentileClipConfig`)
- Sorts values, clips lowest N% and highest N%
- Guarantees at least one survivor
- Default: 10% low, 10% high

### GESD (`GesdConfig`)
- Two-phase: (1) iteratively remove most deviant value, record test statistic R_i
- (2) backward scan comparing R_i against critical values lambda_i
- Uses median + MAD for robust statistics (not textbook mean + stddev)
- Inverse normal approximation (Abramowitz & Stegun) for critical values
- Default: alpha=0.05, max_outliers=25% of n

### None
- Pass-through, no rejection

## Comparison with Industry Standards

### vs PixInsight ImageIntegration

| Feature | This Implementation | PixInsight |
|---------|-------------------|------------|
| Sigma clip center | Median | Median |
| Sigma clip spread | MAD * 1.4826 | MAD * 1.4826 |
| Asymmetric sigma | Yes (sigma_low/high) | Yes (sigma low/high) |
| Winsorized asymmetric | No (single sigma) | Yes (sigma low/high) |
| Linear fit | Sorted-index fit | Sorted-index fit |
| GESD statistics | Median + MAD | Trimmed mean + trimmed stddev |
| GESD relaxation | Not implemented | Yes (default 1.5, multiplies sigma for low pixels) |
| Normalization | None/Global/Multiplicative | Additive/Multiplicative/Additive+Scaling/Local |
| Rejection normalization | Same as combination | Separate from combination normalization |
| Weighting | Manual per-frame weights | Noise eval, PSF signal, PSF SNR, FITS keyword |
| Rejection maps | Not generated | Low/High rejection maps + slope map |
| Large-scale rejection | Not implemented | Layers + growth for satellite/airplane trails |

### vs Siril

| Feature | This Implementation | Siril |
|---------|-------------------|-------|
| Scale estimator | MAD | IKSS (default), MAD (fast mode), sqrt(BWMV) |
| Location estimator | Median | IKSS (default), Median (fast mode) |
| Normalization modes | 3 (None/Global/Mult) | 5 (None/Add/Mult/Add+Scale/Mult+Scale) |
| Weighting formula | Manual | 1/(pscale^2 * bgnoise^2) automatic |
| Rejection maps | No | Yes (low/high, mergeable) |
| MAD clipping | Via sigma clip | Separate algorithm |
| Median sigma clip | No | Yes (replaces rejected with median) |

## Correctness Assessment

### Correct
- **MAD-based sigma**: Uses proper 1.4826 factor for all rejection algorithms
- **Asymmetric sigma clipping**: Separate low/high thresholds, tested extensively
- **Index tracking**: Maintains frame-to-weight mapping through rejection reordering
- **Percentile surviving_range**: Handles edge cases, guarantees >= 1 survivor
- **GESD backward scan**: Correct implementation of the two-phase decision rule
- **Normalization formulas**: Global matches Siril's "additive with scaling"

### Issues

**GESD: Missing asymmetric relaxation (Medium)**
- rejection.rs GesdConfig: PixInsight multiplies sigma by a relaxation factor (default 1.5)
  for low-pixel test statistics, making it much more tolerant of dark outliers.
  This is critical because dark pixels (noise floor) should be rejected less aggressively
  than bright outliers (cosmic rays, satellites).
- Fix: Add `low_relaxation: f32` field (default 1.5) to GesdConfig.

**GESD: Uses median+MAD instead of trimmed mean+trimmed stddev (Medium)**
- rejection.rs GesdConfig::reject: Standard GESD uses mean+stddev. PixInsight improves
  this with trimmed statistics (trimming proportion = max_outliers fraction).
  Median+MAD is more robust but has higher variance for normally-distributed data,
  which can cause over-rejection in small stacks.
- The NIST reference states critical value approximation is accurate for n >= 25.

**Linear fit: Center computed at midpoint position only (Medium)**
- rejection.rs LinearFitClipConfig::reject: `center = a + b * (n / 2.0)` uses a single
  center for all values. The correct approach (matching PixInsight/Siril) is to compare
  each pixel against its own fitted value: `fitted_i = a + b * i`, then reject where
  `|value_i - fitted_i| > sigma * residual_sigma`.
- Current code rejects based on distance from one center point, which is less accurate
  for values far from the median sorted position.

**Linear fit: Residual MAD uses wrong centering (Medium)**
- rejection.rs ~line 349: Computes `(residual_i - residual_at_median_position).abs()`,
  but should compute MAD of all residuals directly (median of absolute deviations
  from the median residual, not from one specific residual).

**Winsorized: Symmetric sigma only (Medium)**
- rejection.rs WinsorizedClipConfig: Single `sigma` field. Both PixInsight and Siril
  support separate sigma_low/sigma_high for asymmetric winsorization.

**Missing additive-only normalization (Low)**
- config.rs Normalization: Has None, Global (additive+scaling), Multiplicative.
  Missing pure additive mode (shift-only, no gain adjustment).
  Siril offers five modes. Pure additive is useful for same-setup sessions with
  varying sky background but consistent gain characteristics.

**Missing rejection maps output (Low)**
- Both PixInsight and Siril generate per-pixel rejection count maps (high/low).
  Critical for diagnosing whether rejection parameters are too aggressive or too lenient.
  PixInsight also generates a slope map for linear fit clipping.

**Missing large-scale rejection (Low)**
- PixInsight has separate large-scale rejection (layers + growth parameters)
  specifically for satellite/airplane trails that span many connected pixels.
  Current implementation handles these only through per-pixel sigma clipping,
  which can miss coherent structures.

## Memory Management

- **In-memory mode**: Chosen when total image data < 75% of available RAM
- **Disk-backed mode**: Per-channel binary files with mmap; hash-based filenames
  allow cache reuse across runs (`keep_cache` option)
- **Chunked processing**: Rows processed in chunks sized to fit in memory;
  adaptive sizing via `compute_optimal_chunk_rows_with_memory()`
- **Parallel I/O**: Loading limited to 3 concurrent threads to cap memory/IO pressure
- **Per-thread scratch**: `ScratchBuffers` allocated once per rayon thread via `for_each_init`
- **Cache cleanup**: `Drop` impl removes cache files unless `keep_cache` is set

### Siril comparison
- Siril uses block-based parallel processing with `_data_block` structures
- Maintains separate rejected/working/original stacks per block
- Both approaches are O(width * height * frames) total; this implementation
  has simpler per-channel pass structure while Siril processes all channels together

## Weighting

- Manual per-frame weights via `StackConfig::weights`
- Weights normalized to sum to 1.0 before use
- Index tracking preserves correct weight-to-value mapping after rejection reordering
- Weighted mean computed via `weighted_mean_indexed()` using frame index mapping
- Missing: automatic noise-based weighting (PixInsight: noise eval via MRS;
  Siril: `1/(pscale^2 * bgnoise^2)`)
- Missing: PSF-based weighting (PixInsight: PSF signal weight, PSF SNR)

## Normalization

- **None**: No adjustment (correct for bias/dark frames)
- **Global**: `gain = ref_mad / frame_mad`, `offset = ref_median - frame_median * gain`
  Matches Siril's "additive with scaling". Best for light frames.
- **Multiplicative**: `gain = ref_median / frame_median`, `offset = 0`
  Best for flat frames where exposure varies.
- Reference frame: always frame 0 (first loaded)
- Statistics: per-channel median and MAD computed via `compute_channel_stats()`
- Missing: separate normalization for rejection vs combination pass
  (PixInsight separates these; important for photometric accuracy)
- Missing: IKSS/BWMV estimators (Siril default; more robust than median+MAD
  but slower). Current MAD-based approach matches Siril's "fast normalization" mode.

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
