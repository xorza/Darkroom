# Star Detection Module

This module implements star candidate detection using thresholding and connected component labeling (CCL). The pipeline follows patterns used by professional astronomical software like SExtractor and Photutils.

## Pipeline Overview

```
detect_stars()
├── create_threshold_mask()  [SIMD: SSE4.1, NEON]
│   └── or create_adaptive_threshold_mask()  [per-pixel sigma]
├── dilate_mask()            [3x3 structuring element]
├── LabelMap::from_mask()    [RLE-based CCL with union-find]
├── extract_candidates()     [Component extraction + deblending]
└── Filter by area and edge margin
```

### Full Detection Pipeline (with matched filter)

```
StarDetector::detect()
├── Apply defect mask (optional)
├── 3x3 median filter (for CFA sensors)
├── BackgroundMap::new()     [Background estimation]
│   └── or new_with_adaptive_sigma()  [Adaptive thresholding]
├── matched_filter()         [SIMD Gaussian convolution]
├── detect_stars()           [Thresholding + CCL]
├── compute_centroid()       [Sub-pixel refinement]
└── Quality filters (SNR, eccentricity, sharpness, etc.)
```

## Implementation Status

### Completed Features

| Feature | Location | Notes |
|---------|----------|-------|
| SIMD threshold mask | `common/threshold_mask/` | SSE4.1, NEON |
| Adaptive threshold mask | `common/threshold_mask/` | Per-pixel sigma, SIMD |
| Matched filtering | `convolution/mod.rs` | Separable Gaussian, SIMD |
| Elliptical PSF support | `convolution/mod.rs` | axis_ratio, angle params |
| RLE-based CCL | `labeling/mod.rs` | ~50% faster than pixel-based |
| Block-based parallel CCL | `labeling/mod.rs` | Strip-based with boundary merge |
| Lock-free union-find | `labeling/mod.rs` | CAS-based atomic operations |
| CTZ run extraction | `labeling/mod.rs` | 10x faster for sparse masks |
| 4/8-connectivity | `labeling/mod.rs` | Configurable via `Connectivity` |
| Multi-threshold deblending | `deblend/multi_threshold.rs` | SExtractor-style |
| Local maxima deblending | `deblend/local_maxima.rs` | Fast, default |
| Touched-label tracking | `mod.rs` | Efficient component collection |
| Early area filtering | `mod.rs` | Skip oversized components |

### Performance

| Benchmark | Median Time | Notes |
|-----------|-------------|-------|
| label_map_from_mask_1k | ~346µs | 1K image, 500 stars |
| label_map_from_mask_4k | ~2ms | 4K image, 2000 stars |
| label_map_from_mask_6k_globular | ~5.5ms | 4K image, 50k stars (dense) |
| detect_stars_6k_50000 | ~234ms | Full pipeline, 6K image |
| matched_filter_4k | ~90ms | Gaussian convolution only |

## Adaptive Thresholding

Adaptive thresholding adjusts the detection sigma based on local image characteristics:

- **Low-contrast regions** (uniform sky): Uses base sigma (default 3.5)
- **High-contrast regions** (nebulae, gradients): Uses higher sigma (up to 6.0)

### Usage

```rust
// Enable with default settings
let config = StarDetectionConfig::default()
    .with_adaptive_threshold();

// Use preset for nebulous fields
let config = StarDetectionConfig::for_nebulous_field();

// Custom configuration
let config = StarDetectionConfig::default()
    .with_adaptive_threshold_config(AdaptiveThresholdConfig {
        base_sigma: 3.5,    // Low-contrast threshold
        max_sigma: 6.0,     // High-contrast threshold
        contrast_factor: 2.0, // Sensitivity to contrast
    });
```

### Algorithm

1. **Tile statistics**: Computes contrast metric (CV = sigma/median) per tile
2. **Adaptive sigma**: `sigma = base + contrast × (max - base)` clamped to [base, max]
3. **Median filter**: 3×3 smoothing of adaptive sigma across tiles
4. **Interpolation**: Bilinear interpolation to per-pixel values
5. **Threshold**: `pixel > background + adaptive_sigma × noise`

## Code Structure

```
detection/
├── mod.rs              # detect_stars(), extract_candidates()
├── tests.rs            # Detection tests
├── bench.rs            # Detection benchmarks
├── plan.md             # Implementation plan and notes
└── labeling/
    ├── mod.rs          # LabelMap, UnionFind, AtomicUnionFind
    ├── tests.rs        # 59 labeling tests
    ├── bench.rs        # Labeling benchmarks
    └── README.md       # Algorithm documentation
```

## API

```rust
// Detect stars with default settings
let candidates = detect_stars(
    &pixels,
    Some(&filtered),  // matched-filtered image (optional)
    &background,
    &config,
);

// Access label map directly
let label_map = LabelMap::from_mask(&binary_mask);
let label_map = LabelMap::from_mask_with_connectivity(
    &binary_mask,
    Connectivity::Eight  // for undersampled PSFs
);
```

## Comparison with Industry Tools

| Feature | SExtractor | Photutils | This Implementation |
|---------|------------|-----------|---------------------|
| CCL Algorithm | Lutz pass | scipy ndi_label | RLE union-find |
| Parallelization | No | No | Yes (strip-based) |
| Deblending | Multi-threshold | Watershed | Both methods |
| Bit-level optimization | No | No | Yes (CTZ) |
| SIMD threshold | No | Via numpy | Yes (SSE4.1/NEON) |
| Adaptive threshold | Local background | Via segmentation | Per-pixel sigma |

## References

- [SExtractor: Software for source extraction](https://aas.aanda.org/articles/aas/pdf/1996/08/ds1060.pdf) - Bertin & Arnouts 1996
- [SIMD RLE CCL algorithms](https://hal.science/hal-02492824) - HAL 2020
- [Optimizing two-pass CCL algorithms](https://www.osti.gov/servlets/purl/887435) - Wu, Otoo, Suzuki
