# Stacking Module Review

Review of `lumos/src/stacking/` against [Siril source](https://gitlab.com/free-astro/siril/-/tree/master/src/stacking) and [PixInsight PCL source](https://github.com/PixInsight/PCL/tree/master/src/modules/processes/ImageIntegration).

## What's Solid

- **Sigma clip uses median + MAD** — matches PixInsight's WinsorizedSigmaClip (the most robust variant). Siril's basic SIGMA uses mean+stddev which is less robust.
- **Compile-time safety**: `CombineMethod::Mean(Rejection)` vs `CombineMethod::Median` — invalid combinations unrepresentable
- **Global normalization** using median + MAD matches PixInsight's "AdditiveWithScaling" mode
- **Multiplicative normalization** for flats is correct (gain only)
- **Weighted mean with index tracking** after rejection — unified path through `weighted_mean_indexed`
- **ScratchBuffers** pre-allocated per rayon thread — no per-pixel allocation (PixInsight `new`/`delete` per pixel)
- **Adaptive storage**: auto in-memory vs disk-backed (mmap) based on RAM
- **Asymmetric sigma clipping**: both low/high thresholds, matches PixInsight/Siril
- **Test coverage** is thorough (~90+ tests)

## Bugs / Wrong Behavior

### 1. Linear fit uses wrong x-values (BUG)

**Our code** (`LinearFitClipConfig::reject`): fits `y = a + b * original_frame_index`, treating pixel values as a time series across frames.

**PixInsight** (`RejectionLinearFit`): sorts values by magnitude, then fits `y = a + b * sorted_index`. Residual sigma from this fit is used as a robust scale estimator. Center is the fit value at the median position.

**Siril** (`line_clipping_float`): identical to PixInsight — sorts values, fits through sorted indices, uses residual sigma.

The purpose of linear fit clipping is NOT to model temporal trends. It fits a line through the **sorted pixel value distribution** to get a robust sigma estimate that accounts for the expected shape of the distribution (which should be roughly linear when sorted). This makes the sigma estimate robust against outliers skewing the mean/stddev.

**Fix**: Sort values (with index co-array), use sorted position as x-values, compute MAD of residuals for sigma, use fit value at median position as center.

**Impact**: Critical — linear fit currently models the wrong thing | **Effort**: Low

### 2. GESD uses mean+stddev instead of median+MAD (quality issue)

**Our code** (`GesdConfig::reject`): test statistic = `|value - mean| / stddev`.

**PixInsight** (`ESDRejection`): test statistic = `|value - median| / MAD`. Uses proper `TDistribution::InverseCDF` for critical values.

**Siril** (`GESDT_float`): uses mean+stddev like ours, but with a precomputed Grubbs lookup table (max 25 frames).

PixInsight's approach is more robust. The mean and stddev can be heavily influenced by the very outliers GESD is trying to detect — the classic masking effect. Using median+MAD for the test statistic makes the detection more reliable.

Our inverse normal approximation for critical values is acceptable (Abramowitz & Stegun is standard), but PixInsight uses proper t-distribution inverse CDF which is more accurate for small sample sizes.

**Fix**: Switch to median+MAD for test statistic computation. Consider proper t-distribution CDF.

**Impact**: Medium — affects GESD quality for small stacks | **Effort**: Low

## Resolved

1. ~~Per-pixel allocation~~ — `ScratchBuffers` pre-allocated per rayon thread
2. ~~`apply_rejection` misleading return~~ — removed; `combine_mean` is the single entry point
3. ~~Median + Rejection non-standard~~ — moved `Rejection` into `CombineMethod::Mean(Rejection)`, compile-time safe
4. ~~HotPixelMap in wrong module~~ — moved to `calibration_masters/`
5. ~~Pairs buffer allocation~~ — replaced with insertion sort + index co-array

## Improvements

### 3. IKSS normalization estimators

Current global normalization uses median + MAD. Both Siril and PixInsight default to **IKSS** (Iterative Kappa-Sigma Sigma-clipping):

1. Clip pixels > k*MAD from median
2. Recompute location/scale on clipped data using BWMV (biweight midvariance)

More robust against bright stars and nebulae skewing statistics. Our median+MAD is Siril's "fast fallback" option.

**Impact**: Quality | **Effort**: Medium

### 4. MAD clipping rejection

Siril offers **MAD Clipping** as a separate rejection method — median + MAD instead of mean + stddev. Minor gap since our sigma clip already uses median+MAD (making our sigma clip equivalent to Siril's MAD clipping).

**Impact**: Low (our sigma clip already covers this) | **Effort**: Low

### 5. Auto weighting schemes

Professional tools compute weights automatically from FWHM, background noise, star count, SNR. PixInsight weights by noise estimates by default.

**Impact**: Feature | **Effort**: High

### 6. Sum stacking method

Simple addition without averaging. Trivial `CombineMethod::Sum` variant.

**Impact**: Completeness | **Effort**: Trivial

### 7. Reference frame selection

Always uses frame 0. Should pick best-quality frame (lowest noise, best FWHM).

**Impact**: Quality | **Effort**: Low (once auto weighting metrics exist)

## Algorithm Comparison Table

| Algorithm | Our Implementation | PixInsight | Siril |
|-----------|-------------------|------------|-------|
| **Sigma Clip** | median + MAD, iterative | Single-pass median+MAD | mean + stddev, iterative |
| **Winsorized** | Clamps outliers to boundary | Iterative reject + recompute median+MAD | mean for sigma, median for center, σ×1.134 |
| **Linear Fit** | Frame index as x (**wrong**) | Sorted-value index as x | Sorted-value index as x |
| **GESD** | mean + stddev, inverse normal approx | median + MAD, t-distribution CDF | mean + stddev, Grubbs lookup table (≤25) |
| **Percentile** | Rank-based (clip N% from ends) | Distance from median | Distance from median (multiplicative) |

## Priority

| # | Item | Impact | Effort |
|---|------|--------|--------|
| 1 | Fix linear fit x-values | Critical | Low |
| 2 | GESD: switch to median+MAD | Medium | Low |
| 3 | IKSS normalization | Quality | Medium |
| 4 | Auto weighting | Feature | High |
| 5 | Sum stacking | Completeness | Trivial |
| 6 | Reference frame selection | Quality | Low |

## Sources

- [PixInsight PCL — IntegrationRejectionEngine.cpp](https://github.com/PixInsight/PCL/blob/master/src/modules/processes/ImageIntegration/IntegrationRejectionEngine.cpp)
- [PixInsight PCL — ImageIntegrationInstance.cpp](https://github.com/PixInsight/PCL/blob/master/src/modules/processes/ImageIntegration/ImageIntegrationInstance.cpp)
- [Siril — rejection_float.c](https://gitlab.com/free-astro/siril/-/blob/master/src/stacking/rejection_float.c)
- [Siril 1.5.0 Stacking Documentation](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)
- [NIST — Generalized ESD Test](https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm)
- [Astropy — sigma_clip](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
