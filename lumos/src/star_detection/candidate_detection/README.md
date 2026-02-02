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
│   └── with adaptive_sigma config    [Adaptive thresholding]
├── determine_effective_fwhm()  [Auto-estimate or manual]
├── matched_filter()         [SIMD Gaussian convolution]
├── detect_stars()           [Thresholding + CCL]
├── compute_centroid()       [Sub-pixel refinement]
└── Quality filters (SNR, eccentricity, sharpness, etc.)
```

## Features

### RLE-based Connected Component Labeling

~50% faster than pixel-based methods (15ms → 5.5ms on 6K globular cluster):

- Run extraction using word-level bit scanning (64-bit fast-paths)
- CTZ-based scanning for mixed words (10x faster for sparse masks)
- Label runs and merge overlapping runs from previous row
- Parallel strip processing with atomic boundary merging
- Lock-free CAS-based union-find for thread safety

### 8-Connectivity Option

For undersampled PSFs where 4-connectivity fragments detections:

```rust
let label_map = LabelMap::from_mask_with_connectivity(
    &binary_mask,
    Connectivity::Eight
);
```

### Adaptive Thresholding

Adjusts detection sigma based on local image characteristics:

- **Low-contrast regions** (uniform sky): Uses base sigma (default 3.5)
- **High-contrast regions** (nebulae, gradients): Uses higher sigma (up to 6.0)

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

**Algorithm:**
1. Computes contrast metric (CV = sigma/median) per tile during background estimation
2. Interpolates per-pixel adaptive sigma alongside background/noise
3. Higher sigma in high-contrast regions, lower in uniform sky
4. SIMD-accelerated threshold mask creation (SSE4.1/NEON)

**Note:** Adaptive thresholding is disabled when matched filter is used because the filter changes noise characteristics. Future work could enable this by scaling adaptive sigma based on filter kernel size.

### Auto-Estimate FWHM

Two-pass detection with automatic FWHM estimation from bright stars:

```rust
// Enable auto-estimation
let config = StarDetectionConfig {
    psf: PsfConfig {
        auto_estimate: true,
        expected_fwhm: 0.0,  // Will be estimated
        ..Default::default()
    },
    ..Default::default()
};

// Check diagnostics for estimated FWHM
let result = detector.detect(&image);
if result.diagnostics.fwhm_was_auto_estimated {
    println!("Estimated FWHM: {:.2} pixels (from {} stars)",
        result.diagnostics.estimated_fwhm,
        result.diagnostics.fwhm_estimation_star_count);
}

// Manual FWHM always takes precedence over auto-estimation
let config = StarDetectionConfig {
    psf: PsfConfig {
        expected_fwhm: 4.0,  // Manual overrides auto
        ..Default::default()
    },
    ..Default::default()
};
```

**Algorithm:**
1. First pass: Detect bright stars without matched filter (2x sigma threshold)
2. Estimate FWHM using robust median + MAD outlier rejection
3. Second pass: Full detection with estimated FWHM matched filter

**Quality filters for estimation:**
- Filters saturated stars (bloated FWHM)
- Filters cosmic rays (high sharpness, small FWHM)
- Filters elongated sources (high eccentricity)
- Uses median + 3×MAD outlier rejection
- Falls back to default 4.0 pixels if insufficient stars

## Performance

| Benchmark | Median Time | Notes |
|-----------|-------------|-------|
| label_map_from_mask_1k | ~346µs | 1K image, 500 stars |
| label_map_from_mask_4k | ~2ms | 4K image, 2000 stars |
| label_map_from_mask_6k_globular | ~5.5ms | 4K image, 50k stars (dense) |
| detect_stars_6k_50000 | ~234ms | Full pipeline, 6K image |
| matched_filter_4k | ~90ms | Gaussian convolution only |

**Overhead:**
- Adaptive thresholding: +24MB memory (adaptive sigma buffer), ~30% background estimation overhead
- Auto-FWHM: ~15-25% overhead (first-pass detection)

## Design Decisions

### Investigated but Not Beneficial

| Optimization | Why Not Beneficial |
|--------------|-------------------|
| Atomic path compression | Strip-based processing keeps trees shallow |
| SIMD run extraction | Most words are zeros (scalar fast-path), dispatch overhead |
| Precomputed lookup tables | CTZ already 10x faster for sparse masks |
| SIMD label flattening | Label mapping is already fast, not a bottleneck |

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
| Adaptive threshold | Local background | Via segmentation | Per-pixel sigma |
| Auto FWHM estimation | No | No | Yes (two-pass) |

## References

- [SExtractor: Software for source extraction](https://aas.aanda.org/articles/aas/pdf/1996/08/ds1060.pdf) - Bertin & Arnouts 1996
- [SIMD RLE CCL algorithms](https://hal.science/hal-02492824) - HAL 2020
- [Optimizing two-pass CCL algorithms](https://www.osti.gov/servlets/purl/887435) - Wu, Otoo, Suzuki
