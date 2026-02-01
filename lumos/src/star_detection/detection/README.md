# Star Detection Module

This module implements star candidate detection using thresholding and connected component labeling (CCL). The pipeline follows patterns used by professional astronomical software like SExtractor and Photutils.

## Pipeline Overview

```
detect_stars()
├── create_threshold_mask()  [SIMD: SSE4.1, NEON]
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

## Code Structure

```
detection/
├── mod.rs              # detect_stars(), extract_candidates()
├── tests.rs            # Detection tests
├── bench.rs            # Detection benchmarks
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

## Investigated Optimizations (Not Beneficial)

These were tested but showed no improvement for star detection workloads:

- **Atomic path compression**: Strip-based processing already keeps trees shallow
- **SIMD run extraction**: Most words are zeros (fast-path in scalar), dispatch overhead negates gains
- **Precomputed lookup tables**: CTZ already achieves 10x for sparse masks

See `labeling/README.md` for detailed benchmarks.

## References

- [SExtractor: Software for source extraction](https://aas.aanda.org/articles/aas/pdf/1996/08/ds1060.pdf) - Bertin & Arnouts 1996
- [SIMD RLE CCL algorithms](https://hal.science/hal-02492824) - HAL 2020
- [Optimizing two-pass CCL algorithms](https://www.osti.gov/servlets/purl/887435) - Wu, Otoo, Suzuki
