# stack.rs Review vs Industry Best Practices

Review of `stack.rs` against [Siril](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html), [PixInsight](https://chaoticnebula.com/pixinsight-image-integration/), and [Clark Vision](https://clarkvision.com/articles/image-stacking-methods/).

## What's Solid

- **Rejection algorithm coverage**: sigma clip (symmetric + asymmetric), winsorized, linear fit, percentile, GESD — matches Siril's full set
- **Global normalization** using median + MAD is the standard approach (Siril calls this "Additive with scaling")
- **Multiplicative normalization** for flats is correct (gain only, no offset)
- **Weighted mean with index tracking** after rejection is correctly implemented — unified path through `weighted_mean_indexed` for all rejection variants that reorder values
- **ScratchBuffers** pre-allocated per rayon thread — no per-pixel allocation
- **Test coverage** is thorough, especially the weight-alignment tests

## Resolved

### Per-pixel allocation (was #1)
Fixed — `ScratchBuffers` (indices, floats_a, floats_b) are pre-allocated once per rayon thread via `process_chunked`'s `for_each_init` and reused across all pixels.

### `apply_rejection` misleading return (was #5)
Fixed — `apply_rejection` removed. `combine_mean` is only called from the `Mean` path; the `Median` path calls `reject()` directly then `median_f32_mut`.

## Remaining Improvements

### 1. Median + Rejection combination is non-standard (API clarity)

The `CombineMethod::Median` arm applies rejection then takes median of survivors. Standard practice (per Siril, PixInsight) is that rejection is paired with **mean** combination, not median. Taking median after rejection discards information. Consider removing `Median+Rejection` or documenting it as non-standard.

**Effort**: Low

### 2. IKSS-style robust normalization estimators (quality)

Current global normalization uses median + MAD for location and scale estimation. Siril defaults to **IKSS** (Iterative Kappa-Sigma Sigma-clipping) estimators:

1. Clip pixels > 6*MAD from median
2. Recompute location/scale on clipped data using BWMV (biweight midvariance)

This is more robust against bright stars and nebulae skewing the statistics. The current median+MAD approach works well for most cases but can be biased by large bright objects in the field.

**Effort**: Medium

### 3. Missing: MAD clipping rejection

Siril offers **MAD Clipping** as a separate rejection method — uses MAD instead of standard deviation, making it more robust for noisy data (especially infrared). Minor gap since winsorized sigma clip covers similar ground.

**Effort**: Low

### 4. Missing weighting schemes

Current weights are raw user-provided values. Professional tools compute weights automatically from:
- **FWHM** (focus quality)
- **Background noise** level
- **Star count**
- **SNR** estimates

More of a feature gap than a code issue.

**Effort**: High

### 5. Missing: Sum stacking method

Siril supports **sum stacking** (simple addition without averaging) for planetary/8-bit data. Niche but trivial to add.

**Effort**: Trivial

## Priority

| Item | Impact | Effort |
|------|--------|--------|
| Median+Rejection clarity | API clarity | Low |
| IKSS estimators | Quality (more robust normalization) | Medium |
| MAD clipping | Feature completeness | Low |
| Auto weighting schemes | Feature (user convenience) | High |
| Sum stacking | Feature completeness | Trivial |

## Sources

- [Siril 1.5.0 Stacking Documentation](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)
- [PixInsight Image Integration](https://chaoticnebula.com/pixinsight-image-integration/)
- [Clark Vision: Image Stacking Methods Compared](https://clarkvision.com/articles/image-stacking-methods/)
- [Siril Statistics (IKSS)](https://free-astro.org/index.php?title=Siril:Statistics)
