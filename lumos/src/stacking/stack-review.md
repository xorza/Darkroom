# stack.rs Review vs Industry Best Practices

Review of `stack.rs` against [Siril](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html), [PixInsight](https://chaoticnebula.com/pixinsight-image-integration/), and [Clark Vision](https://clarkvision.com/articles/image-stacking-methods/).

## What's Solid

- **Rejection algorithm coverage**: sigma clip (symmetric + asymmetric), winsorized, linear fit, percentile, GESD — matches Siril's full set
- **Global normalization** using median + MAD is the standard approach (Siril calls this "Additive with scaling")
- **Multiplicative normalization** for flats is correct (gain only, no offset)
- **Weighted mean with index tracking** after rejection is correctly implemented — a subtle but important detail that ensures surviving values pair with their original frame weights
- **Test coverage** is thorough, especially the weight-alignment tests

## Potential Improvements

### 1. Per-pixel allocation in rejection closures (performance)

In `dispatch_stacking`, the `Mean+rejection` and `Median+rejection` arms allocate a `Vec<usize>` **per pixel**:

```rust
let mut indices: Vec<usize> = (0..values.len()).collect();
```

This is called millions of times (once per pixel per channel). The indices vector is small (frame count), but the allocation churn could be noticeable. Consider pre-allocating in the closure capture or passing it through the chunked processing.

### 2. IKSS-style robust normalization estimators (quality)

Current global normalization uses median + MAD for location and scale estimation. Siril defaults to **IKSS** (Iterative Kappa-Sigma Sigma-clipping) estimators:

1. Clip pixels > 6*MAD from median
2. Recompute location/scale on clipped data using BWMV (biweight midvariance)

This is more robust against bright stars and nebulae skewing the statistics. The current median+MAD approach works well for most cases but can be biased by large bright objects in the field.

### 3. Missing: MAD clipping rejection

Siril offers **MAD Clipping** as a separate rejection method — uses MAD instead of standard deviation, making it more robust for noisy data (especially infrared). Minor gap since winsorized sigma clip covers similar ground.

### 4. Median + Rejection combination is unusual

The `(CombineMethod::Median, rejection)` arm applies rejection then takes median of survivors. Standard practice (per Siril, PixInsight) is that rejection is paired with **mean** combination, not median. Taking median after rejection discards information. The `Median` method should typically be used **without** rejection (as the `(Median, None)` arm already does). The `(Median, rejection)` combination is unusual and may confuse users — consider removing it or documenting it as non-standard.

### 5. `apply_rejection` returns mean for `Rejection::None`

```rust
Rejection::None => RejectionResult {
    value: math::mean_f32(values),
    remaining_count: values.len(),
}
```

This computes mean even when the caller might want median. The callers in `dispatch_stacking` handle `Rejection::None` via separate match arms (so this is dead code), but it's semantically misleading.

### 6. Missing weighting schemes

Current weights are raw user-provided values. Professional tools compute weights automatically from:
- **FWHM** (focus quality)
- **Background noise** level
- **Star count**
- **SNR** estimates

More of a feature gap than a code issue.

### 7. Missing: Sum stacking method

Siril supports **sum stacking** (simple addition without averaging) for planetary/8-bit data. Niche but trivial to add.

## Priority

| Item | Impact | Effort |
|------|--------|--------|
| Per-pixel allocation | Performance (could be significant for large stacks) | Low |
| IKSS estimators | Quality (more robust normalization) | Medium |
| Remove/document Median+Rejection | API clarity | Low |
| MAD clipping | Feature completeness | Low |
| Auto weighting schemes | Feature (user convenience) | High |
| Sum stacking | Feature completeness | Trivial |

## Sources

- [Siril 1.5.0 Stacking Documentation](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)
- [PixInsight Image Integration](https://chaoticnebula.com/pixinsight-image-integration/)
- [Clark Vision: Image Stacking Methods Compared](https://clarkvision.com/articles/image-stacking-methods/)
- [Siril Statistics (IKSS)](https://free-astro.org/index.php?title=Siril:Statistics)
