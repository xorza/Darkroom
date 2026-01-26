# Star Detection Benchmark Plan

This document describes the benchmark suite for the star detection module, focusing on NEON vs Scalar performance comparisons.

## Available Benchmarks

| Benchmark | File | Description |
|-----------|------|-------------|
| `star_detection_background` | `benches/star_detection_background.rs` | Background estimation algorithms |
| `star_detection_centroid` | `benches/star_detection_centroid.rs` | Centroid computation and profile fitting |
| `star_detection_convolution` | `benches/star_detection_convolution.rs` | Gaussian convolution and matched filtering |
| `star_detection_cosmic_ray` | `benches/star_detection_cosmic_ray.rs` | Cosmic ray detection via Laplacian |
| `star_detection_deblend` | `benches/star_detection_deblend.rs` | Star deblending algorithms |
| `star_detection_detection` | `benches/star_detection_detection.rs` | Threshold mask and connected components |
| `median_filter` | `benches/median_filter.rs` | 3x3 median filter for Bayer artifacts |

## NEON vs Scalar Comparison Benchmarks

### 1. Background Estimation

```bash
cargo bench -p lumos --features bench --bench star_detection_background
```

**SIMD vs Scalar benchmarks:**

| Benchmark Group | Operations | Sizes |
|-----------------|------------|-------|
| `background_simd_vs_scalar` | `sum_and_sum_sq`, `sum_abs_dev` | 1024, 4096, 16384, 65536 elements |
| `background_interpolation` | segment interpolation (scalar vs simd) | 64, 256, 1024, 4096 elements |

**Functions compared:**
- `sum_and_sum_sq_simd()` vs scalar loop
- `sum_abs_deviations_simd()` vs scalar loop
- `interpolate_segment_simd()` vs scalar interpolation

### 2. Convolution

```bash
cargo bench -p lumos --features bench --bench star_detection_convolution
```

**SIMD vs Scalar benchmarks:**

| Benchmark Group | Operation | Sizes |
|-----------------|-----------|-------|
| `convolution_simd_vs_scalar` | `convolve_row` | 256, 512, 1024, 2048 width |

**Functions compared:**
- `convolve_row_simd()` (NEON/SSE) vs scalar row convolution

### 3. Cosmic Ray Detection

```bash
cargo bench -p lumos --features bench --bench star_detection_cosmic_ray
```

**SIMD vs Scalar benchmarks:**

| Benchmark Group | Operation | Sizes |
|-----------------|-----------|-------|
| `cosmic_ray_simd_vs_scalar` | Laplacian computation | 512x512, 1024x1024, 2048x2048 |

**Functions compared:**
- `compute_laplacian_simd()` vs `compute_laplacian_scalar()`

### 4. Median Filter

```bash
cargo bench -p lumos --features bench --bench median_filter
```

**SIMD vs Scalar benchmarks:**

| Benchmark Group | Operation | Sizes |
|-----------------|-----------|-------|
| `median_filter_row` | 3x3 median row processing | 256, 512, 1024, 2048 width |

**Functions compared:**
- `median_filter_row_simd()` (NEON sorting network) vs `median_filter_row_scalar()`

## Non-SIMD Benchmarks

### Centroid

```bash
cargo bench -p lumos --features bench --bench star_detection_centroid
```

Benchmarks weighted centroid refinement, Gaussian fitting, and Moffat fitting at various stamp sizes (15x15, 21x21, 31x31).

### Deblend

```bash
cargo bench -p lumos --features bench --bench star_detection_deblend
```

Benchmarks local maxima deblending and multi-threshold deblending with varying star pair counts and separations.

## Quick Commands

### Run all SIMD vs Scalar comparisons:

```bash
# Background SIMD comparisons
cargo bench -p lumos --features bench --bench star_detection_background -- simd_vs_scalar
cargo bench -p lumos --features bench --bench star_detection_background -- interpolation

# Convolution SIMD comparison
cargo bench -p lumos --features bench --bench star_detection_convolution -- simd_vs_scalar

# Cosmic ray SIMD comparison
cargo bench -p lumos --features bench --bench star_detection_cosmic_ray -- simd_vs_scalar

# Median filter SIMD comparison
cargo bench -p lumos --features bench --bench median_filter -- median_filter_row
```

### Run all benchmarks:

```bash
cargo bench -p lumos --features bench
```

### Run specific benchmark with detailed output:

```bash
cargo bench -p lumos --features bench --bench star_detection_cosmic_ray -- --verbose
```

## Benchmark Results

### Background Estimation Results (Apple Silicon M-series)

**Full Pipeline (background_estimation):**

| Image Size | Tile 32 | Tile 64 | Tile 128 |
|------------|---------|---------|----------|
| 512x512 | 739 µs (355 Melem/s) | 840 µs (312 Melem/s) | 901 µs (291 Melem/s) |
| 2048x2048 | 14.9 ms (282 Melem/s) | 13.7 ms (306 Melem/s) | 13.0 ms (323 Melem/s) |
| 4096x4096 | 49.1 ms (341 Melem/s) | 49.5 ms (339 Melem/s) | 56.0 ms (299 Melem/s) |

**SIMD vs Scalar Comparison (sum_and_sum_sq):**

| Size | Scalar | SIMD | Speedup |
|------|--------|------|---------|
| 1024 | 872 ns (1.17 Gelem/s) | 232 ns (4.42 Gelem/s) | **3.76x** |
| 4096 | 3.65 µs (1.12 Gelem/s) | 1.16 µs (3.52 Gelem/s) | **3.14x** |
| 16384 | 15.1 µs (1.09 Gelem/s) | 4.86 µs (3.37 Gelem/s) | **3.10x** |
| 65536 | 60.0 µs (1.09 Gelem/s) | 19.6 µs (3.35 Gelem/s) | **3.07x** |

**SIMD vs Scalar Comparison (sum_abs_dev):**

| Size | Scalar | SIMD | Speedup |
|------|--------|------|---------|
| 1024 | 807 ns (1.27 Gelem/s) | 180 ns (5.70 Gelem/s) | **4.48x** |
| 4096 | 3.59 µs (1.14 Gelem/s) | 865 ns (4.73 Gelem/s) | **4.15x** |
| 16384 | 14.8 µs (1.11 Gelem/s) | 3.63 µs (4.52 Gelem/s) | **4.08x** |
| 65536 | 59.6 µs (1.10 Gelem/s) | 14.9 µs (4.41 Gelem/s) | **4.01x** |

**Interpolation SIMD vs Scalar:**

| Size | Scalar | SIMD | Speedup |
|------|--------|------|---------|
| 64 | 33.6 ns (1.90 Gelem/s) | 11.1 ns (5.78 Gelem/s) | **3.04x** |
| 256 | 123 ns (2.08 Gelem/s) | 53.9 ns (4.75 Gelem/s) | **2.28x** |
| 1024 | 486 ns (2.11 Gelem/s) | 242 ns (4.22 Gelem/s) | **2.01x** |
| 4096 | 1.92 µs (2.13 Gelem/s) | 1.01 µs (4.07 Gelem/s) | **1.91x** |

---

### Centroid Results (Apple Silicon M-series)

**Centroid Computation:**

| Operation | Time |
|-----------|------|
| refine_centroid_21x21 | 753 ns |
| compute_metrics_21x21 | 1.17 µs |
| compute_stamp_radius | 14 ns |

**Batch Processing:**

| Batch Size | Time | Throughput |
|------------|------|------------|
| 10 stars | 63.5 µs | 157 K/s |
| 100 stars | 665 µs | 150 K/s |
| 500 stars | 3.46 ms | 144 K/s |

**Gaussian 2D Fitting:**

| Stamp Size | Time |
|------------|------|
| 15x15 | 8.59 µs |
| 21x21 | 71.1 µs |
| 31x31 | 173 µs |

**Moffat 2D Fitting:**

| Stamp Size | Fixed Beta | Variable Beta |
|------------|------------|---------------|
| 15x15 | 15.9 µs | 32.9 µs |
| 21x21 | 30.4 µs | 167 µs |
| 31x31 | 74.2 µs | 233 µs |

---

### Convolution Results (Apple Silicon M-series)

**SIMD vs Scalar Row Convolution:**

| Width | Scalar | SIMD | Speedup |
|-------|--------|------|---------|
| 256 | 3.41 µs (75 Melem/s) | 410 ns (624 Melem/s) | **8.32x** |
| 512 | 6.87 µs (74 Melem/s) | 808 ns (634 Melem/s) | **8.51x** |
| 1024 | 13.7 µs (75 Melem/s) | 1.32 µs (775 Melem/s) | **10.4x** |
| 2048 | 27.4 µs (75 Melem/s) | 2.54 µs (806 Melem/s) | **10.8x** |

---

### Cosmic Ray Detection Results (Apple Silicon M-series)

**SIMD vs Scalar Laplacian Computation:**

| Image Size | Scalar | SIMD | Speedup |
|------------|--------|------|---------|
| 512x512 | 385 µs (681 Melem/s) | 66.4 µs (3.95 Gelem/s) | **5.80x** |
| 1024x1024 | 1.57 ms (668 Melem/s) | 245 µs (4.29 Gelem/s) | **6.42x** |
| 2048x2048 | 6.25 ms (671 Melem/s) | 1.71 ms (2.45 Gelem/s) | **3.66x** |

**Full Cosmic Ray Detection Pipeline:**

| Image Size | Time | Throughput |
|------------|------|------------|
| 512x512 | 3.39 ms | 77 Melem/s |
| 1024x1024 | 13.7 ms | 77 Melem/s |
| 2048x2048 | 55.7 ms | 75 Melem/s |

---

### Deblend Results (Apple Silicon M-series)

**Local Maxima Deblending:**

| Star Pairs | Separation | Time | Throughput |
|------------|------------|------|------------|
| 10 pairs | 15 px | 27.0 µs | 370 K/s |
| 50 pairs | 12 px | 148 µs | 338 K/s |
| 100 pairs | 10 px | 313 µs | 320 K/s |

**Multi-threshold Deblending:**

| Star Pairs | Separation | Time | Throughput |
|------------|------------|------|------------|
| 10 pairs | 15 px | 30.1 ms | 333 elem/s |
| 50 pairs | 12 px | 133 ms | 376 elem/s |

---

### Detection Results (Apple Silicon M-series)

**Threshold Mask Creation:**

| Image Size | Time | Throughput |
|------------|------|------------|
| 512x512 | 109 µs | 2.40 Gelem/s |
| 1024x1024 | 447 µs | 2.34 Gelem/s |
| 2048x2048 | 1.79 ms | 2.35 Gelem/s |

**Mask Dilation:**

| Image Size | Radius 1 | Radius 2 | Radius 3 |
|------------|----------|----------|----------|
| 512x512 | 193 µs (1.36 Gelem/s) | 281 µs (932 Melem/s) | 413 µs (634 Melem/s) |
| 1024x1024 | 553 µs (1.90 Gelem/s) | 641 µs (1.64 Gelem/s) | 780 µs (1.35 Gelem/s) |
| 2048x2048 | 2.00 ms (2.09 Gelem/s) | 2.09 ms (2.01 Gelem/s) | 2.22 ms (1.89 Gelem/s) |

**Connected Components:**

| Image Size | Stars | Time | Throughput |
|------------|-------|------|------------|
| 512x512 | 50 | 252 µs | 1.04 Gelem/s |
| 1024x1024 | 200 | 993 µs | 1.06 Gelem/s |
| 2048x2048 | 800 | 2.87 ms | 1.46 Gelem/s |
| 4096x4096 | 3200 | 11.4 ms | 1.48 Gelem/s |

**Extract Candidates:**

| Image Size | Stars | Simple | Multi-threshold |
|------------|-------|--------|-----------------|
| 512x512 | 50 | 186 µs | 27.7 ms |
| 1024x1024 | 200 | 726 µs | ~100 ms |

---

### Median Filter Results (Apple Silicon M-series)

**Full 3x3 Median Filter:**

| Image Size | Time | Throughput |
|------------|------|------------|
| 512x512 | 91.0 µs | 2.88 Gelem/s |
| 1024x1024 | 265 µs | 3.96 Gelem/s |
| 4096x4096 | 3.80 ms | 4.42 Gelem/s |

**SIMD vs Scalar Row Processing:**

| Width | Scalar | SIMD | Speedup |
|-------|--------|------|---------|
| 256 | 309 ns (828 Melem/s) | 193 ns (1.33 Gelem/s) | **1.60x** |
| 512 | 605 ns (847 Melem/s) | 375 ns (1.37 Gelem/s) | **1.61x** |
| 1024 | 1.20 µs (854 Melem/s) | 719 ns (1.42 Gelem/s) | **1.67x** |
| 2048 | 2.38 µs (859 Melem/s) | 1.41 µs (1.45 Gelem/s) | **1.69x** |

---

## Measured SIMD Speedups Summary

| Operation | Measured NEON Speedup | Notes |
|-----------|----------------------|-------|
| Sum/Sum-squared | **3.1-3.8x** | Best at small sizes |
| Sum absolute deviations | **4.0-4.5x** | Excellent scaling |
| Interpolation | **1.9-3.0x** | Best at small sizes |
| Row convolution | **8.3-10.8x** | Exceeds expectations |
| Laplacian | **3.7-6.4x** | Memory-bound at large sizes |
| Median filter (sorting network) | **1.6-1.7x** | Expected for sorting network |

## Platform Support

- **aarch64 (Apple Silicon)**: NEON SIMD
- **x86_64**: SSE2/SSE4.1/AVX2 SIMD
- **Other**: Scalar fallback

The benchmarks automatically use the best available SIMD implementation for the current platform.
