# Star Detection Module - Implementation Notes

## Current Implementation Status

The star detection module is feature-complete for image registration use cases. All major algorithms from DAOPHOT and SExtractor have been implemented.

### Implemented Features

| Feature | Module | Status |
|---------|--------|--------|
| Matched filtering (Gaussian convolution) | `convolution/` | Done |
| Tile grid background estimation | `background/` | Done |
| Iterative background refinement | `background/` | Done |
| Sigma-clipped statistics | `background/` | Done |
| Connected component labeling | `detection/` | Done |
| Morphological dilation | `detection/` | Done |
| Local maxima deblending | `deblend/` | Done |
| Multi-threshold deblending (SExtractor-style) | `deblend/` | Done |
| Iterative weighted centroid | `centroid/` | Done |
| 2D Gaussian fitting (L-M) | `centroid/gaussian_fit.rs` | Done |
| 2D Moffat fitting (L-M) | `centroid/moffat_fit.rs` | Done |
| Sharpness metric | `centroid/` | Done |
| Roundness metrics (GROUND/SROUND) | `centroid/` | Done |
| L.A.Cosmic Laplacian SNR | `cosmic_ray/` | Done |
| 3x3 median filter | `median_filter/` | Done |
| FWHM outlier filtering | `mod.rs` | Done |
| Duplicate removal | `mod.rs` | Done |

### SIMD Implementations

| Module | AVX2/SSE | NEON | Integrated | Notes |
|--------|----------|------|------------|-------|
| `convolution/simd/` | Done | Done | Yes | Primary hotspot, well optimized |
| `background/simd/` | Done | Done | No | Prepared but not needed (fast enough) |
| `detection/simd/` | Placeholder | Placeholder | No | Not a bottleneck |
| `median_filter/simd/` | Placeholder | Placeholder | No | Already 2.6 Gelem/s without SIMD |
| `cosmic_ray/simd/` | Placeholder | Placeholder | No | Full-image mode rarely used |

### Benchmark Coverage

All benchmarks are now wired up and functional.

**Standalone criterion benchmarks** (in `lumos/benches/`):
| File | Status |
|------|--------|
| `star_detection_background.rs` | Done |
| `star_detection_centroid.rs` | Done |
| `star_detection_convolution.rs` | Done |
| `star_detection_cosmic_ray.rs` | Done |
| `star_detection_deblend.rs` | Done |
| `star_detection_detection.rs` | Done |
| `median_filter.rs` | Done |

Run with: `cargo bench -p lumos --features bench --bench <name>`

---

## Benchmark Results (2048x2048 image, ~800 stars)

Measured on Linux x86_64 with AVX2/FMA.

### Per-Operation Performance

| Module | Operation | Time | Throughput | Notes |
|--------|-----------|------|------------|-------|
| **background** | estimate_background (tile 64) | 8.8ms | 479 Melem/s | |
| **convolution** | gaussian_convolve (sigma 2) | 7.7ms | 546 Melem/s | SIMD optimized |
| **convolution** | matched_filter (fwhm 4) | 16.7ms | 251 Melem/s | Includes background subtraction |
| **detection** | create_threshold_mask | 0.6ms | Very fast | Simple comparison |
| **detection** | dilate_mask (radius 1) | 1.6ms | 2.6 Gelem/s | |
| **detection** | connected_components | 5.6ms | 751 Melem/s | Union-find |
| **detection** | detect_stars | 12.4ms | 337 Melem/s | Full pipeline |
| **centroid** | compute_centroid (500 stars) | 4.5ms | 111 Kstars/s | Per-star processing |
| **centroid** | fit_gaussian_2d (21px stamp) | 80µs | Per star | L-M optimization |
| **centroid** | fit_moffat_2d_fixed (21px) | 34µs | Per star | Faster than variable beta |
| **cosmic_ray** | compute_laplacian | 5.9ms | 706 Melem/s | 3x3 convolution |
| **cosmic_ray** | detect_cosmic_rays | 69ms | 60 Melem/s | Includes fine structure |
| **deblend** | local_maxima (100 pairs) | 262µs | 381 Kpairs/s | Fast, default method |
| **deblend** | multi_threshold (50 pairs) | 103ms | 486 pairs/s | Slow, optional |
| **median_filter** | median_filter_3x3 (4096x4096) | 6.3ms | 2.6 Gelem/s | Very fast |

### Full Pipeline Estimate

For a typical 4K image (4096x4096) with ~2000 stars:
- Median filter: ~6ms
- Background estimation: ~25ms
- Matched filter: ~50ms
- Detection: ~50ms
- Centroid computation: ~18ms (2000 stars × 9µs)
- **Total: ~150ms**

This is acceptable for interactive use and batch processing.

---

## Optimization Analysis

### Bottlenecks Identified

1. **Multi-threshold deblending** (103ms for 50 pairs): Expected for tree-based algorithm. Use local_maxima instead (400x faster) unless crowded field accuracy is critical.

2. **Cosmic ray full-image detection** (69ms): Dominated by fine_structure computation. Per-star Laplacian SNR (used in `find_stars`) is much faster.

3. **Matched filter** (17ms for 2K): Already SIMD-optimized. Further gains would require algorithmic changes (e.g., FFT convolution for large kernels).

### Optimizations NOT Needed

1. **Median filter SIMD**: Already achieves 2.6 Gelem/s, which is near memory bandwidth limit.

2. **Detection SIMD**: Threshold comparison is simple and fast. Connected components is algorithm-bound, not compute-bound.

3. **Background SIMD integration**: Current scalar implementation is fast enough (~25ms for 4K). SIMD code exists but integration not worth the complexity.

### Future Optimization Opportunities (Low Priority)

If profiling shows bottlenecks in real-world usage:

1. **Buffer reuse**: Allocate scratch buffers once and reuse across pipeline stages.

2. **Parallel centroiding**: Use rayon to process stars in parallel (currently sequential).

3. **Fast Gaussian centroid**: Replace iterative refinement with closed-form solution (research shows 15x speedup possible).

4. **Integral images**: For background estimation tile statistics.

---

## Known Limitations

1. **No gain parameter**: SNR calculation assumes background-dominated regime
2. **No read noise handling**: Affects faint star detection accuracy
3. **Fixed CFA handling**: Median filter always applied, even for mono sensors
4. **No WCS awareness**: Cannot use catalog positions for verification
5. **No defect map support**: Hot columns, bad pixels not explicitly handled
6. **No PSF photometry**: For crowded fields, aperture photometry fails (not needed for registration)

---

## Module Structure

```
star_detection/
├── mod.rs                    # Public API, Star struct, find_stars()
├── notes.md                  # This file
│
├── background/               # Background estimation
│   ├── mod.rs               # estimate_background(), BackgroundMap
│   ├── bench.rs             # Criterion benchmarks
│   ├── tests.rs             # Unit tests
│   └── simd/                # SIMD implementations (prepared, not integrated)
│
├── convolution/              # Matched filter / Gaussian convolution
│   ├── mod.rs               # matched_filter(), gaussian_convolve()
│   ├── bench.rs             # Criterion benchmarks
│   ├── tests.rs             # Unit tests
│   └── simd/                # SIMD implementations (integrated)
│
├── detection/                # Thresholding and connected components
│   ├── mod.rs               # detect_stars(), connected_components()
│   ├── bench.rs             # Criterion benchmarks
│   ├── tests.rs             # Unit tests
│   └── simd/                # Placeholder for future optimization
│
├── deblend/                  # Star deblending
│   ├── mod.rs               # DeblendConfig
│   ├── local_maxima.rs      # Simple deblending (fast, default)
│   ├── multi_threshold.rs   # SExtractor-style tree deblending (slow, optional)
│   ├── bench.rs             # Criterion benchmarks
│   └── tests.rs             # Unit tests
│
├── centroid/                 # Centroid computation
│   ├── mod.rs               # compute_centroid(), refine_centroid()
│   ├── gaussian_fit.rs      # 2D Gaussian L-M fitting
│   ├── moffat_fit.rs        # 2D Moffat L-M fitting
│   ├── linear_solver.rs     # 5x5/6x6 Gaussian elimination
│   ├── bench.rs             # Criterion benchmarks
│   └── tests.rs             # Unit tests
│
├── cosmic_ray/               # Cosmic ray detection
│   ├── mod.rs               # detect_cosmic_rays()
│   ├── laplacian.rs         # L.A.Cosmic Laplacian computation
│   ├── fine_structure.rs    # Median-based noise estimation
│   ├── bench.rs             # Criterion benchmarks
│   ├── tests.rs             # Unit tests
│   └── simd/                # Placeholder for future optimization
│
├── median_filter/            # Median filtering
│   ├── mod.rs               # median_filter_3x3()
│   ├── bench.rs             # Criterion benchmarks
│   ├── tests.rs             # Unit tests
│   └── simd/                # Placeholder (not needed, already fast)
│
├── tests.rs                  # Integration tests
└── visual_tests/             # Debug visualization (test-only)
```

---

## References

### Foundational Papers
- [Stetson 1987 - DAOPHOT](https://ui.adsabs.harvard.edu/abs/1987PASP...99..191S) - Crowded field photometry
- [Bertin & Arnouts 1996 - SExtractor](https://aas.aanda.org/articles/aas/abs/1996/08/ds1060/ds1060.html) - Source extraction
- [van Dokkum 2001 - L.A.Cosmic](https://ui.adsabs.harvard.edu/abs/2001PASP..113.1420V) - Cosmic ray rejection
- [Moffat 1969](https://ui.adsabs.harvard.edu/abs/1969A&A.....3..455M) - Moffat profile

### Modern Implementations
- [Photutils DAOStarFinder](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
- [Photutils IRAFStarFinder](https://photutils.readthedocs.io/en/stable/api/photutils.detection.IRAFStarFinder.html)
- [SEP (Source Extraction and Photometry)](https://sep.readthedocs.io/)

### Centroid Accuracy
- [Fast Gaussian Centroiding](https://link.springer.com/article/10.1007/s40295-015-0034-4) - 15x faster than iterative
- [Sub-pixel centroid on FPGA](https://link.springer.com/article/10.1007/s11554-014-0408-z) - 1/33 pixel accuracy
- [Star Centroiding Research](https://pmc.ncbi.nlm.nih.gov/articles/PMC6163372/) - Method comparison
