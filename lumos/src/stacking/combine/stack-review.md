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

### ~~1. Linear fit uses wrong x-values~~ (FIXED)

Fixed: now sorts values with index co-array, fits `y = a + b * sorted_index`, uses MAD of residuals for sigma, fit value at median position as center. First pass uses initial median+MAD (robust starting point), subsequent passes refine with linear fit. Matches PixInsight/Siril.

### ~~2. GESD uses mean+stddev instead of median+MAD~~ (FIXED)

Fixed: switched to median+MAD for test statistics (more robust against masking effect). Implemented proper two-phase approach: Phase 1 iteratively finds most deviant value and records test statistic; Phase 2 backward scans comparing test statistics against critical values to determine actual outlier count. Matches PixInsight.

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
| **Linear Fit** | Sorted-value index as x, MAD of residuals | Sorted-value index as x | Sorted-value index as x |
| **GESD** | median + MAD, two-phase, inverse normal approx | median + MAD, t-distribution CDF | mean + stddev, Grubbs lookup table (≤25) |
| **Percentile** | Rank-based (clip N% from ends) | Distance from median | Distance from median (multiplicative) |

## Priority

| # | Item | Impact | Effort |
|---|------|--------|--------|
| ~~1~~ | ~~Fix linear fit x-values~~ | ~~Critical~~ | ~~Done~~ |
| ~~2~~ | ~~GESD: switch to median+MAD~~ | ~~Medium~~ | ~~Done~~ |
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
