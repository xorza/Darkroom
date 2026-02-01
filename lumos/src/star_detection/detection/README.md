# Star Detection Module - Research and Implementation Notes

## Overview

This module implements star candidate detection using thresholding and connected component labeling (CCL). The pipeline follows the established pattern used by professional astronomical software like SExtractor and Photutils.

## Current Implementation Review

### Pipeline Structure

```
detect_stars()
├── create_threshold_mask() / create_threshold_mask_filtered()  [SIMD optimized]
├── dilate_mask() - 3x3 structuring element
├── LabelMap::from_mask() - Connected component labeling
├── extract_candidates() - Component data collection + deblending
└── Filter by area and edge margin
```

### Full Detection Pipeline (with matched filter)

```
StarDetector::detect()
├── Apply defect mask (optional)
├── 3x3 median filter (for CFA sensors)
├── BackgroundMap::new() - Background estimation
├── matched_filter() - Gaussian convolution [SIMD optimized]
├── detect_stars() - Thresholding + CCL
├── compute_centroid() - Sub-pixel refinement
└── Quality filters (SNR, eccentricity, sharpness, etc.)
```

### Implemented Features (Done)

| Feature | Status | Location |
|---------|--------|----------|
| **SIMD threshold mask** | Done | `common/threshold_mask/` (SSE4.1, NEON) |
| **Matched filtering** | Done | `convolution/mod.rs` (separable Gaussian, SIMD) |
| **Elliptical PSF support** | Done | `convolution/mod.rs` (axis_ratio, angle params) |
| **Word-level bit scanning** | Done | `labels.rs` (trailing_zeros for sparse masks) |
| **Block-based parallel CCL** | Done | `labels.rs` (strip-based with boundary merge) |
| **Lock-free atomic union-find** | Done | `labels.rs` (CAS operations) |
| **Touched-label tracking** | Done | `mod.rs` (collect_component_data) |
| **Early area filtering** | Done | `mod.rs` (skip oversized components) |
| **Multi-threshold deblending** | Done | `deblend/multi_threshold.rs` (SExtractor-style) |
| **Local maxima deblending** | Done | `deblend/local_maxima.rs` (fast, default) |

### Strengths

1. **SIMD-accelerated threshold mask** - Uses SSE4.1/NEON for vectorized threshold comparison with bit-packing.

2. **Matched filter convolution** - Separable Gaussian with SIMD (O(n*k) instead of O(n*k^2)), boosts SNR for faint star detection.

3. **Word-level bit scanning** in `labels.rs` using `trailing_zeros()` to skip background pixels efficiently.

4. **Block-based parallel CCL** with boundary merging - divides image into horizontal strips, labels in parallel, then merges at boundaries.

5. **Lock-free atomic union-find** for parallel label merging with CAS operations.

6. **Touched-label tracking** in `collect_component_data()` - avoids O(num_labels) reset per chunk by only tracking labels actually seen.

7. **Early area filtering** - marks oversized components (max_area + 1) to skip expensive deblending.

### Areas for Potential Improvement

1. **Decision tree optimization for neighbor checks**: The current implementation checks all neighbors. Wu, Otoo, and Suzuki's research shows that using a decision tree can reduce neighbor accesses by ~2x by exploiting local topology.

2. **Run-length encoding (RLE)**: Research shows SIMD RLE-based CCL can outperform direct pixel methods by 1.7-1.9x. The current word-level scanning is a step in this direction but doesn't use full RLE.

3. **Path compression during find**: The `atomic_find` function doesn't perform path compression (only reads). Adding atomic path compression could reduce tree depth over multiple finds.

4. **8-connectivity vs 4-connectivity**: Current uses 4-connectivity. 8-connectivity may reduce fragmentation for some PSFs but requires more neighbor checks.

## Industry Best Practices

### SExtractor Algorithm

SExtractor (Bertin & Arnouts 1996) is the gold standard for astronomical source extraction:

- Uses **Lutz (1979) pass algorithm** for extracting 8-connected contiguous pixels
- Applies **convolution filtering** before thresholding (matched filter for point sources) - **Done**
- **Multi-threshold deblending**: Re-thresholds each component at 30 exponentially-spaced levels - **Done (configurable n_thresholds)**
- **Contrast-based deblending**: Evaluates mean surface brightness contribution from neighbors - **Done**

### Photutils Algorithm

Photutils (Astropy) uses scipy's `ndi_label()` with optimizations:

- **Cutout-based filtering**: Creates localized segments (~10x faster than full-array operations)
- **Label mapping via lookup table**: Avoids expensive array operations during relabeling
- **Early termination**: Returns immediately if no pixels exceed threshold

### Connected Component Labeling Research

Key papers and their contributions:

| Algorithm | Key Innovation | Speedup |
|-----------|---------------|---------|
| Wu-Otoo-Suzuki (Two-pass) | Decision tree + optimized union-find | ~2x neighbor access reduction |
| SIMD RLE (HAL 2020) | Run-length + SIMD vectorization | 1.7-1.9x on AVX2/AVX512 |
| Block-based GPU (2017) | Local merge + boundary-only analysis | 1.3x+ |
| Grana (BBDT) | Block-based with decision trees | State-of-art for dense images |

## Comparison with Our Implementation

| Feature | SExtractor | Photutils | Our Implementation |
|---------|------------|-----------|-------------------|
| CCL Algorithm | Lutz pass | scipy ndi_label | Union-find with word-level scanning |
| Parallelization | No | No | Yes (strip-based) |
| Deblending | Multi-threshold (30 levels) | Watershed | Multi-threshold (configurable) |
| Bit-level optimization | No | No | Yes (trailing_zeros) |
| SIMD threshold | No | Via numpy | Yes (SSE4.1/NEON) |
| Matched filter | Yes | Optional | Yes (SIMD separable Gaussian) |

## Potential Optimizations (Prioritized)

### High Impact (Not Yet Implemented)

1. **Run-based CCL**: Convert to RLE representation, then merge runs. Research shows 1.7-1.9x speedup possible.

2. **Decision tree for neighbor access**: Implement Wu-Otoo-Suzuki style decision tree to reduce neighbor checks from 4 to ~2 average.

3. **Adaptive local thresholding**: Instead of single sigma threshold, use local adaptive thresholds for images with variable nebulosity.

### Medium Impact

4. **Atomic path compression**: Add path compression to `atomic_find` during labeling phase.

5. **Better strip sizing**: Current 64-row minimum might not be optimal for all cache sizes.

6. **Vectorized label flattening**: The final label mapping pass could use SIMD.

7. **8-connectivity option**: Add optional 8-connectivity mode for better handling of undersampled PSFs.

### Low Impact (Already Well Optimized)

8. ~~SIMD threshold mask~~ - **Done**
9. ~~Matched filtering~~ - **Done**
10. ~~Touched-label tracking~~ - **Done**
11. ~~Word-level bit scanning~~ - **Done**
12. ~~Early area filtering~~ - **Done**

## Detection Precision Improvements

For improving detection accuracy (not just speed):

| Improvement | Impact | Status |
|-------------|--------|--------|
| Matched filtering | High (2-3x more faint stars) | Done |
| Iterative background refinement | Medium | Done (configurable iterations) |
| Multi-threshold deblending | High for crowded fields | Done |
| Sub-pixel centroiding | High for astrometry | Done (weighted moments, Gaussian/Moffat fit) |
| Cosmic ray rejection | Medium | Done (sharpness filter) |
| FWHM outlier rejection | Medium | Done (MAD-based) |
| Adaptive local background | Medium | Partial (annulus method available) |

## References

- [SExtractor: Software for source extraction](https://aas.aanda.org/articles/aas/pdf/1996/08/ds1060.pdf) - Bertin & Arnouts 1996
- [Photutils detect_sources](https://photutils.readthedocs.io/en/stable/api/photutils.segmentation.detect_sources.html) - Astropy
- [SIMD RLE CCL algorithms](https://hal.science/hal-02492824) - HAL 2020
- [Optimizing two-pass CCL algorithms](https://www.osti.gov/servlets/purl/887435) - Wu, Otoo, Suzuki
- [GPU Union-Find CCL](https://arxiv.org/abs/1708.08180) - 2017
- [DRUID: Persistent homology for source detection](https://arxiv.org/abs/2410.22508) - 2024

## Benchmarks (Current)

| Benchmark | Median Time | Notes |
|-----------|-------------|-------|
| detect_stars_6k_50000 | ~232ms | Full pipeline, 6144x6144, 50k stars |
| detect_6k_globular_cluster | ~365ms | Dense field with crowding |
| label_map_from_mask_6k_globular | ~15ms | CCL only |
| extract_candidates_6k_dense | ~206ms | Component extraction + deblending |
| matched_filter_4k | ~90ms | Gaussian convolution only |

## Code Structure

```
detection/
├── mod.rs          # Main detect_stars() and extract_candidates()
├── labels.rs       # LabelMap with union-find CCL
├── bench.rs        # Benchmarks
└── tests.rs        # Unit tests

convolution/
├── mod.rs          # matched_filter(), gaussian_convolve()
├── simd/           # SIMD implementations (SSE, NEON)
├── bench.rs        # Convolution benchmarks
└── tests.rs        # Convolution tests

common/threshold_mask/
├── mod.rs          # create_threshold_mask() with SIMD dispatch
├── sse.rs          # SSE4.1 implementation
├── neon.rs         # ARM NEON implementation
└── tests.rs        # Threshold tests
```

## Implementation Notes

### Why Union-Find over Lutz/Two-Pass?

The union-find approach was chosen for:
1. Better parallelization - strips can be labeled independently
2. Simpler boundary merging - just union labels at boundaries
3. Word-level bit scanning integration - natural fit with sparse mask processing

### Why Dilation Before CCL?

The 1-pixel dilation (3x3 structuring element) connects:
- Bayer pattern artifacts (demosaiced images may have gaps)
- Noise-induced fragmentation in faint stars
- Sub-pixel PSF sampling effects

Radius 1 minimizes star merging while connecting fragmented detections.

### Deblending Strategy

Two modes supported:
1. **Local maxima** (default): Fast, finds peaks and partitions by Voronoi
2. **Multi-threshold**: SExtractor-style, builds tree at multiple threshold levels

Multi-threshold is more accurate for crowded fields but slower.

### Matched Filter Details

The matched filter (Gaussian convolution) improves faint star detection by:
1. Subtracting background first
2. Convolving with Gaussian kernel matching expected PSF (FWHM)
3. Boosting SNR by ~sqrt(n_pixels) for point sources

Supports elliptical PSFs via `psf_axis_ratio` and `psf_angle` parameters.
Uses separable convolution with SIMD acceleration (SSE/NEON).
