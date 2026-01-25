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

| Module | AVX2/SSE | NEON | Integrated |
|--------|----------|------|------------|
| `convolution/simd/` | Done | Done | Yes |
| `background/simd/` | Done | Done | No (prepared, not in hot path) |
| `detection/simd/` | Empty | Empty | No |
| `median_filter/simd/` | Empty | Empty | No |
| `cosmic_ray/simd/` | Empty | Empty | No |

### Benchmark Coverage

**Integrated module benchmarks** (in `src/star_detection/*/bench.rs`):
| Module | Criterion benchmarks | Status |
|--------|---------------------|--------|
| `background/bench.rs` | `estimate_background` | Done |
| `convolution/bench.rs` | `gaussian_kernel_1d`, `gaussian_convolve`, `matched_filter` | Done |
| `detection/bench.rs` | `threshold_mask`, `dilate_mask`, `connected_components`, `extract_candidates`, `detect_stars` | Done |
| `centroid/bench.rs` | `refine_centroid`, `compute_metrics`, `fit_gaussian_2d`, `fit_moffat_2d` | Done |
| `cosmic_ray/bench.rs` | `compute_laplacian`, `detect_cosmic_rays` | Done (struct-based) |
| `deblend/bench.rs` | `local_maxima`, `multi_threshold` | Done (struct-based) |
| `median_filter/bench.rs` | `median_filter_3x3` | Done |

**Standalone criterion benchmarks** (in `lumos/benches/`):
| File | Status | Notes |
|------|--------|-------|
| `star_detection_background.rs` | Done | Calls `lumos::bench::background::benchmarks` |
| `star_detection_centroid.rs` | Empty | Needs implementation |
| `star_detection_convolution.rs` | Empty | Needs implementation |
| `star_detection_cosmic_ray.rs` | Empty | Needs implementation |
| `star_detection_deblend.rs` | Empty | Needs implementation |
| `star_detection_detection.rs` | Empty | Needs implementation |

---

## Pending Work

### 1. Benchmark Infrastructure (Priority: High)

The module-level benchmarks exist but the standalone criterion benchmark files in `lumos/benches/` are mostly empty. These need to be wired up.

**Tasks:**
- [ ] Wire up `star_detection_centroid.rs` to call `centroid::bench::benchmarks`
- [ ] Wire up `star_detection_convolution.rs` to call `convolution::bench::benchmarks`
- [ ] Wire up `star_detection_cosmic_ray.rs` (requires criterion adapter)
- [ ] Wire up `star_detection_deblend.rs` (requires criterion adapter)
- [ ] Wire up `star_detection_detection.rs` to call `detection::bench::benchmarks`
- [ ] Add `median_filter` standalone benchmark

**Note:** The `cosmic_ray/bench.rs` and `deblend/bench.rs` use a struct-based approach instead of criterion directly. Either convert them to criterion format or create adapter functions.

### 2. SIMD Implementations (Priority: Medium)

Several SIMD placeholder files are empty. Priority based on profiling:

**High impact (data-parallel pixel operations):**
- [ ] `median_filter/simd/` - 3x3 median is O(n) and called on full image
- [ ] `detection/simd/` - Threshold comparison is simple but high-volume

**Medium impact:**
- [ ] `cosmic_ray/simd/` - 3x3 Laplacian convolution

**Low impact (already fast or not in hot path):**
- [ ] Integrate `background/simd/` into hot path (currently prepared but unused)

### 3. Test Coverage Improvements (Priority: Medium)

Current test coverage is good but could be improved:

- [ ] Add property-based tests for centroid accuracy (known ground truth)
- [ ] Add regression tests with real astronomical images
- [ ] Add edge case tests for very crowded fields
- [ ] Add tests for undersampled PSFs (FWHM < 2 pixels)

### 4. Potential Optimizations (Priority: Low)

Based on research and profiling, these optimizations could improve performance:

**Memory optimizations:**
- [ ] Reuse scratch buffers in `find_stars()` pipeline
- [ ] Use `Vec::with_capacity()` consistently where sizes are known
- [ ] Consider arena allocator for temporary pixel buffers

**Algorithm optimizations:**
- [ ] Fast Gaussian centroid: explicit closed-form expressions instead of iterative refinement (15x faster per research)
- [ ] Integral image for faster tile statistics in background estimation
- [ ] Huang algorithm for sliding median (O(1) per pixel instead of O(9))

**Cache optimizations:**
- [ ] Add `#[repr(align(64))]` to per-thread accumulators in parallel loops
- [ ] Verify rayon chunk sizes align with cache lines

### 5. Potential Simplifications (Priority: Low)

- [ ] Consider removing `visual_tests/` module if not actively used (877 lines)
- [ ] Consolidate `linear_solver.rs` if only used by Gaussian/Moffat fitting
- [ ] Review if both Gaussian and Moffat fitting are needed (Gaussian may suffice for registration)

---

## Known Limitations

1. **No gain parameter**: SNR calculation assumes background-dominated regime
2. **No read noise handling**: Affects faint star detection accuracy
3. **Fixed CFA handling**: Median filter always applied, even for mono sensors
4. **No WCS awareness**: Cannot use catalog positions for verification
5. **No defect map support**: Hot columns, bad pixels not explicitly handled
6. **No PSF photometry**: For crowded fields, aperture photometry fails (not needed for registration)

---

## Future Enhancements (Not Planned)

These are documented for reference but not currently planned:

### PSF Photometry
For crowded fields (star clusters, galactic bulge), PSF fitting would enable:
- Simultaneous fitting of overlapping sources
- Fainter star detection via iterative subtraction
- Better photometry accuracy

Reference: [Stetson 1987 DAOPHOT](https://ui.adsabs.harvard.edu/abs/1987PASP...99..191S)

### Wavelet-Based Detection
Wavelet transforms provide multi-scale decomposition:
- Better separation of stars from extended sources
- Natural handling of varying PSF across field
- More robust to complex backgrounds

Reference: [Starlet Transform](http://jstarck.free.fr/Chapter_Starlet2011.pdf)

### Deep Learning Approaches
Recent advances (2023-2025) show CNNs can outperform classical methods:
- Neural network centroids: ~10x improvement over CoG
- Transformer architectures for detection and deblending
- Self-supervised learning for PSF estimation

References:
- [DeepDISC](https://arxiv.org/html/2307.05826) - Detectron2-based instance segmentation
- [DRUID](https://arxiv.org/html/2410.22508v1) - Persistent homology for source detection

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
│       ├── mod.rs
│       ├── sse.rs
│       └── neon.rs
│
├── convolution/              # Matched filter / Gaussian convolution
│   ├── mod.rs               # matched_filter(), gaussian_convolve()
│   ├── bench.rs             # Criterion benchmarks
│   ├── tests.rs             # Unit tests
│   └── simd/                # SIMD implementations (integrated)
│       ├── mod.rs
│       ├── sse.rs
│       └── neon.rs
│
├── detection/                # Thresholding and connected components
│   ├── mod.rs               # detect_stars(), connected_components()
│   ├── bench.rs             # Criterion benchmarks
│   ├── tests.rs             # Unit tests
│   └── simd/                # Empty - pending implementation
│       ├── mod.rs
│       ├── sse.rs
│       └── neon.rs
│
├── deblend/                  # Star deblending
│   ├── mod.rs               # DeblendConfig
│   ├── local_maxima.rs      # Simple deblending
│   ├── multi_threshold.rs   # SExtractor-style tree deblending
│   ├── bench.rs             # Benchmark helpers
│   └── tests.rs             # Unit tests
│
├── centroid/                 # Centroid computation
│   ├── mod.rs               # compute_centroid(), refine_centroid()
│   ├── gaussian_fit.rs      # 2D Gaussian L-M fitting
│   ├── moffat_fit.rs        # 2D Moffat L-M fitting
│   ├── linear_solver.rs     # 5x5/6x6 Gaussian elimination
│   ├── bench.rs             # Criterion benchmarks
│   └── tests.rs             # Unit tests (1328 lines)
│
├── cosmic_ray/               # Cosmic ray detection
│   ├── mod.rs               # detect_cosmic_rays()
│   ├── laplacian.rs         # L.A.Cosmic Laplacian computation
│   ├── fine_structure.rs    # Median-based noise estimation
│   ├── bench.rs             # Benchmark helpers
│   ├── tests.rs             # Unit tests
│   └── simd/                # Empty - pending implementation
│       ├── mod.rs
│       ├── sse.rs
│       └── neon.rs
│
├── median_filter/            # Median filtering
│   ├── mod.rs               # median_filter_3x3()
│   ├── bench.rs             # Criterion benchmarks
│   ├── tests.rs             # Unit tests (738 lines)
│   └── simd/                # Empty - pending implementation
│       ├── mod.rs
│       ├── sse.rs
│       └── neon.rs
│
├── tests.rs                  # Integration tests
└── visual_tests/             # Debug visualization (test-only)
    ├── mod.rs
    ├── synthetic.rs
    ├── debug_steps.rs
    └── subpixel_accuracy.rs
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

### SIMD Optimization
- [SimSIMD](https://github.com/ashvardanian/simsimd) - Up to 200x faster dot products
- [SIMDeez](https://docs.rs/simdeez/latest/simdeez/) - Write-once SIMD abstraction
