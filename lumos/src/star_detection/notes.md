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

## Known Limitations & Implementation Plan

This section documents current limitations and provides a concrete plan to address each one.

### 1. No Gain Parameter (SNR Calculation)

**Current state**: SNR is calculated as `flux / (noise * sqrt(aperture_area))`, which assumes background-dominated regime. This underestimates noise for bright stars.

**Correct CCD noise equation**:
```
SNR = flux / sqrt(flux/gain + npix × (σ_sky² + σ_read²/gain²))
```

Where:
- `flux` = source counts (ADU)
- `gain` = electrons per ADU (e-/ADU)
- `npix` = number of pixels in aperture
- `σ_sky` = sky background noise per pixel
- `σ_read` = read noise in electrons

**Implementation plan**:

| Step | Task | Files | Priority |
|------|------|-------|----------|
| 1.1 | Add `gain` and `read_noise` fields to `StarDetectionConfig` | `mod.rs` | Medium |
| 1.2 | Update `compute_metrics()` to use full CCD equation | `centroid/mod.rs` | Medium |
| 1.3 | Make gain/read_noise optional (default to background-dominated) | `mod.rs` | Medium |
| 1.4 | Add tests comparing SNR with/without gain correction | `centroid/tests.rs` | Medium |

**API change**:
```rust
pub struct StarDetectionConfig {
    // ... existing fields ...
    
    /// Camera gain in electrons per ADU (e-/ADU).
    /// Used for accurate SNR calculation. Set to None for background-dominated estimate.
    pub gain: Option<f32>,
    
    /// Read noise in electrons.
    /// Used for accurate SNR calculation. Set to None to ignore.
    pub read_noise: Option<f32>,
}
```

---

### 2. No Read Noise Handling

**Current state**: Detection threshold only considers sky noise (`background + k × σ_sky`). Read noise is not included.

**Impact**: Faint star detection is less accurate, especially for low-gain cameras or short exposures where read noise dominates.

**Implementation plan**:

| Step | Task | Files | Priority |
|------|------|-------|----------|
| 2.1 | Add combined noise calculation: `σ_total = sqrt(σ_sky² + σ_read²)` | `background/mod.rs` | Low |
| 2.2 | Update `BackgroundMap.noise` to include read noise if provided | `background/mod.rs` | Low |
| 2.3 | Pass read_noise from config through the pipeline | `mod.rs`, `detection/mod.rs` | Low |

**Note**: This is coupled with limitation #1 (gain parameter). Implement together.

---

### 3. Fixed CFA Handling (Median Filter Always Applied)

**Current state**: `find_stars()` always applies 3x3 median filter to smooth Bayer pattern artifacts. This is unnecessary for monochrome sensors and adds ~6ms overhead.

**Implementation plan**:

| Step | Task | Files | Priority |
|------|------|-------|----------|
| 3.1 | Add `is_cfa: bool` field to `StarDetectionConfig` | `mod.rs` | High |
| 3.2 | Skip median filter when `is_cfa == false` | `mod.rs:find_stars()` | High |
| 3.3 | Default `is_cfa` to `true` for backward compatibility | `mod.rs` | High |

**API change**:
```rust
pub struct StarDetectionConfig {
    // ... existing fields ...
    
    /// Whether the image has a Color Filter Array (Bayer/X-Trans).
    /// When true, applies median filter to remove CFA artifacts.
    /// Set to false for monochrome sensors to skip the filter (~6ms faster).
    pub is_cfa: bool,
}
```

**Estimated improvement**: 6ms faster for mono sensors on 4K images.

---

### 4. No WCS Awareness

**Current state**: Cannot cross-match detected stars with catalogs. This would enable:
- Astrometric verification (are we detecting real stars?)
- Photometric calibration
- Rejection of asteroids/satellites

**Implementation plan**:

| Step | Task | Files | Priority |
|------|------|-------|----------|
| 4.1 | Define `CatalogStar` struct with RA/Dec and magnitude | New: `catalog.rs` | Low |
| 4.2 | Add optional WCS transform to config (or as separate function) | `mod.rs` or new file | Low |
| 4.3 | Implement `cross_match_catalog(stars, catalog, wcs, tolerance)` | New: `catalog.rs` | Low |
| 4.4 | Return matched/unmatched statistics | `catalog.rs` | Low |

**Note**: WCS parsing itself is out of scope (use external crate like `wcslib` bindings or `fitsio` WCS support). This feature adds catalog matching capability once WCS is available.

**API sketch**:
```rust
pub struct CatalogStar {
    pub ra: f64,    // Right Ascension (degrees)
    pub dec: f64,   // Declination (degrees)
    pub mag: f32,   // Magnitude
}

pub struct CrossMatchResult {
    pub matched: Vec<(Star, CatalogStar)>,
    pub unmatched_detected: Vec<Star>,
    pub unmatched_catalog: Vec<CatalogStar>,
}

pub fn cross_match_catalog(
    stars: &[Star],
    catalog: &[CatalogStar],
    wcs: &impl WorldCoordinateSystem,
    tolerance_arcsec: f64,
) -> CrossMatchResult;
```

---

### 5. No Defect Map Support

**Current state**: Hot pixels, dead pixels, and bad columns are not explicitly handled. They may be detected as cosmic rays (if sharp) or cause false detections.

**Implementation plan**:

| Step | Task | Files | Priority |
|------|------|-------|----------|
| 5.1 | Define `DefectMap` struct (hot pixels, dead pixels, bad columns) | New: `defects.rs` | Medium |
| 5.2 | Add optional `defect_map` to `StarDetectionConfig` | `mod.rs` | Medium |
| 5.3 | Mask defect pixels before detection (set to NaN or background) | `mod.rs:find_stars()` | Medium |
| 5.4 | Reject stars whose centroids fall on defects | `centroid/mod.rs` | Medium |
| 5.5 | Add `Star.near_defect: bool` flag for diagnostics | `mod.rs` | Low |

**API sketch**:
```rust
#[derive(Debug, Clone, Default)]
pub struct DefectMap {
    /// Hot pixel coordinates (x, y).
    pub hot_pixels: Vec<(usize, usize)>,
    /// Dead pixel coordinates (x, y).
    pub dead_pixels: Vec<(usize, usize)>,
    /// Bad columns (x coordinate).
    pub bad_columns: Vec<usize>,
    /// Bad rows (y coordinate).
    pub bad_rows: Vec<usize>,
}

pub struct StarDetectionConfig {
    // ... existing fields ...
    
    /// Optional defect map for masking bad pixels.
    pub defect_map: Option<DefectMap>,
}
```

**Implementation detail**: Convert `DefectMap` to a boolean mask once at start of `find_stars()`, then use it to:
1. Replace defect pixels with local median before processing
2. Skip candidates whose peak falls on a defect
3. Flag stars within N pixels of a defect

---

### 6. No PSF Photometry

**Current state**: Uses aperture photometry (sum of pixels in stamp). This fails in crowded fields where stellar profiles overlap.

**Note**: PSF photometry is NOT needed for image registration (the primary use case). This is documented as a limitation for photometry use cases only.

**Implementation plan** (if needed for future photometry features):

| Step | Task | Files | Priority |
|------|------|-------|----------|
| 6.1 | Build empirical PSF from isolated stars | New: `psf.rs` | Very Low |
| 6.2 | Implement simultaneous PSF fitting (DAOPHOT-style) | New: `psf_photometry.rs` | Very Low |
| 6.3 | Iterative detection: fit, subtract, detect fainter stars | `psf_photometry.rs` | Very Low |

**Complexity**: High. This is essentially implementing DAOPHOT's crowded-field photometry. Consider using existing tools (photutils, DAOPHOT) for photometry instead.

**Recommendation**: Mark as "out of scope" for registration pipeline. Document that users needing accurate photometry in crowded fields should use dedicated photometry software.

---

### Implementation Priority Summary

| Limitation | Priority | Effort | Impact |
|------------|----------|--------|--------|
| 3. CFA handling | **High** | Low | 6ms speedup for mono, easy win |
| 1. Gain parameter | Medium | Medium | More accurate SNR for bright stars |
| 2. Read noise | Medium | Low | Coupled with #1 |
| 5. Defect map | Medium | Medium | Prevents false detections |
| 4. WCS/catalog | Low | Medium | Nice-to-have for verification |
| 6. PSF photometry | Very Low | Very High | Out of scope for registration |

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
