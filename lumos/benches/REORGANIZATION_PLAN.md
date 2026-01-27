# Benchmark Reorganization Plan

**Status: COMPLETED**

## Summary

The benchmarks have been reorganized from a flat structure into categorized subdirectories grouped by algorithm stage. This makes it easier to find, run, and maintain benchmarks for specific parts of the pipeline.

## New Directory Structure

```
benches/
├── REORGANIZATION_PLAN.md      # This file
├── run_benches.sh              # Updated runner script with category support
├── results/                    # Benchmark results
│
├── star_detection/             # Star Detection Pipeline
│   ├── convolution.rs          # Gaussian kernels, image convolution
│   ├── background.rs           # Background estimation
│   ├── threshold.rs            # Threshold mask creation
│   ├── deblend.rs              # Multi-threshold deblending
│   ├── centroid.rs             # Centroid refinement
│   └── cosmic_ray.rs           # Cosmic ray detection
│
├── image_processing/           # Image Processing
│   ├── demosaic_bayer.rs       # Bayer demosaicing
│   ├── demosaic_xtrans.rs      # X-Trans demosaicing
│   ├── hot_pixels.rs           # Hot pixel correction
│   └── median_filter.rs        # Median filtering
│
├── registration/               # Registration
│   └── pipeline.rs             # Full registration pipeline
│
├── stacking/                   # Stacking
│   ├── mean.rs                 # Mean stacking
│   ├── median.rs               # Median stacking
│   └── sigma_clipped.rs        # Sigma-clipped mean stacking
│
├── math/                       # Core Math Operations
│   └── operations.rs           # Sum, accumulate, scale, statistics
│
└── pipeline/                   # End-to-End Pipeline
    └── full_pipeline.rs        # Complete processing pipeline
```

## Running Benchmarks

Use the updated `run_benches.sh` script:

```bash
# Show help
./benches/run_benches.sh --help

# Run all benchmarks in a category
./benches/run_benches.sh star_detection
./benches/run_benches.sh stacking

# Run multiple categories
./benches/run_benches.sh star_detection stacking math

# Run all benchmarks
./benches/run_benches.sh all

# Run a specific benchmark
./benches/run_benches.sh star_detection_convolution
```

Or use cargo directly:

```bash
# Run a specific benchmark
cargo bench --features bench --bench star_detection_convolution

# Run all benchmarks (note: may require significant memory due to LTO)
cargo bench --features bench
```

## Benchmark Categories

| Category | Benchmarks | Tests |
|----------|------------|-------|
| `star_detection` | 6 | Convolution, background estimation, thresholding, deblending, centroid refinement, cosmic ray detection |
| `image_processing` | 4 | Bayer demosaic, X-Trans demosaic, hot pixel correction, median filter |
| `registration` | 1 | Full registration pipeline (star matching, RANSAC, warping) |
| `stacking` | 3 | Mean, median, sigma-clipped mean stacking |
| `math` | 1 | Core math operations (sum, accumulate, scale, statistics) |
| `pipeline` | 1 | End-to-end astrophotography pipeline |

## Implementation Variants Tested

Each benchmark tests implementations where available:

| Stage | Scalar | SIMD | GPU |
|-------|--------|------|-----|
| Convolution | Yes | Yes | - |
| Background | Yes | Yes | - |
| Threshold | Yes | Yes | - |
| Deblend | Yes | - | - |
| Centroid | Yes | - | - |
| Cosmic Ray | Yes | Yes | - |
| Demosaic Bayer | Yes | - | - |
| Demosaic X-Trans | Yes | Yes | - |
| Median Filter | Yes | Yes | - |
| Hot Pixels | Yes | - | - |
| Registration | Yes | - | Yes |
| Mean Stack | Yes | Yes | - |
| Median Stack | Yes | - | - |
| Sigma Clipped | Yes | - | - |
| Math Operations | Yes | Yes | - |
| Full Pipeline | Yes | - | Yes |

## Atomic Benchmark Guidelines

Each benchmark should:

1. **Test ONE algorithm stage** - No combined operations
2. **Use consistent test data** - Same image sizes across benchmarks
3. **Report throughput** - Elements/second or bytes/second
4. **Include all implementations** - Scalar, SIMD, GPU where available
5. **Use descriptive IDs** - `BenchmarkId::new("impl_type", "size")`
6. **Be reproducible** - Deterministic test data generation

## Standard Image Sizes

- **Small**: 512x512 (quick iteration)
- **Medium**: 1024x1024, 2048x2048 (typical sub-frames)
- **Large**: 4096x4096, 6000x4000 (full-frame sensors)

## Next Step

**Create `README.md`** with:
- How to run benchmarks
- Current performance baselines
- How to add new benchmarks
- Interpretation of results
