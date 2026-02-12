# Stacking Module Review

Review of `lumos/src/stacking/` against [Siril](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html), [PixInsight ImageIntegration](https://chaoticnebula.com/pixinsight-image-integration/), and [Clark Vision](https://clarkvision.com/articles/image-stacking-methods/).

## What's Solid

- **Rejection algorithm coverage**: sigma clip (symmetric + asymmetric), winsorized, linear fit, percentile, GESD — matches Siril's full set
- **Compile-time safety**: `CombineMethod::Mean(Rejection)` vs `CombineMethod::Median` — invalid combinations (Median + Rejection) are unrepresentable
- **Global normalization** using median + MAD is the standard approach (Siril calls this "Additive with scaling")
- **Multiplicative normalization** for flats is correct (gain only, no offset)
- **Weighted mean with index tracking** after rejection — unified path through `weighted_mean_indexed` for all rejection variants that reorder values
- **ScratchBuffers** pre-allocated per rayon thread — no per-pixel allocation
- **Adaptive storage**: auto-switches in-memory vs disk-backed (mmap) based on available RAM (75% threshold)
- **Generic design**: `StackableImage` trait allows multiple image types
- **Test coverage** is thorough (~90+ tests): weight-alignment, algorithm correctness, edge cases, normalization modes

## Resolved

1. ~~Per-pixel allocation~~ — `ScratchBuffers` pre-allocated per rayon thread
2. ~~`apply_rejection` misleading return~~ — removed; `combine_mean` is the single entry point
3. ~~Median + Rejection non-standard~~ — moved `Rejection` into `CombineMethod::Mean(Rejection)`, compile-time safe
4. ~~HotPixelMap in wrong module~~ — moved to `calibration_masters/`
5. ~~Pairs buffer allocation~~ — replaced with insertion sort + index co-array

## Improvements

### 1. IKSS normalization estimators (quality)

Current global normalization uses median + MAD for location and scale estimation. Both Siril and PixInsight default to **IKSS** (Iterative Kappa-Sigma Sigma-clipping) estimators:

1. Clip pixels > k*MAD from median
2. Recompute location/scale on clipped data using BWMV (biweight midvariance)

More robust against bright stars and nebulae skewing the statistics. The current median+MAD approach works well for most cases but can be biased by large bright objects.

Siril note: "By default, Siril uses IKSS estimators of location and scale to compute normalisation, but for long sequences, you can opt in for faster estimators based on median and median absolute deviation."

**Impact**: Quality (more robust normalization) | **Effort**: Medium

### 2. MAD clipping rejection

Siril offers **MAD Clipping** as a separate rejection method — uses MAD instead of standard deviation in the clipping criterion, making it more robust for noisy data (especially infrared). Minor gap since winsorized sigma clip covers similar ground.

**Impact**: Feature completeness | **Effort**: Low

### 3. Auto weighting schemes

Current weights are raw user-provided values. Professional tools compute weights automatically from:
- **FWHM** (focus quality)
- **Background noise** level
- **Star count** / roundness
- **SNR** estimates

PixInsight weights by noise estimates by default. Siril supports FWHM, roundness, noise, and star count weighting.

**Impact**: Feature (user convenience) | **Effort**: High

### 4. Sum stacking method

Siril supports **sum stacking** (simple addition without averaging) for planetary/8-bit data. Niche but trivial to add as a `CombineMethod::Sum` variant.

**Impact**: Feature completeness | **Effort**: Trivial

### 5. Reference frame selection

Current implementation always uses frame 0 as the normalization reference. Siril and PixInsight select the reference frame based on quality metrics (lowest noise, best FWHM). This matters when frame 0 happens to be a poor frame — the normalization target would be suboptimal.

**Impact**: Quality | **Effort**: Low (once auto weighting metrics exist)

## Priority

| Item | Impact | Effort | Depends on |
|------|--------|--------|------------|
| IKSS estimators | Quality | Medium | — |
| MAD clipping | Completeness | Low | — |
| Sum stacking | Completeness | Trivial | — |
| Reference frame selection | Quality | Low | #3 (auto weighting) |
| Auto weighting schemes | Feature | High | — |

## Sources

- [Siril 1.5.0 Stacking Documentation](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)
- [Siril 1.0 Rejection Algorithms](https://free-astro.org/siril_doc-en/co/Average_Stacking_With_Rejection__1.html)
- [Siril Normalization Algorithms](https://free-astro.org/siril_doc-en/co/Average_Stacking_With_Rejection__2.html)
- [PixInsight Image Integration](https://chaoticnebula.com/pixinsight-image-integration/)
- [PixInsight ESD Rejection](https://www.adamblockstudios.com/articles/extreme-studentized-deviate-pixel-rejection-esd)
- [Clark Vision: Image Stacking Methods Compared](https://clarkvision.com/articles/image-stacking-methods/)
