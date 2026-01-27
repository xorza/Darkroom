# Star Detection Benchmarks Analysis

**Date:** 2026-01-26

## Overview

This document analyzes the performance of star detection subsystems in the lumos crate. Benchmarks cover background estimation, centroid computation, convolution, cosmic ray detection, detection/thresholding, and deblending.

---

## 1. Background Estimation

### Summary Table

| Image Size | Tile Size | Time (median) | Throughput |
|------------|-----------|---------------|------------|
| 512x512 | 32 | 658.74 µs | 397.95 Melem/s |
| 512x512 | 64 | 678.87 µs | 386.15 Melem/s |
| 512x512 | 128 | 957.58 µs | 273.76 Melem/s |
| 2048x2048 | 32 | 22.643 ms | 185.24 Melem/s |
| 2048x2048 | 64 | 21.789 ms | 192.50 Melem/s |
| 2048x2048 | 128 | 22.106 ms | 189.74 Melem/s |
| 4096x4096 | 32 | 66.565 ms | 252.04 Melem/s |
| 4096x4096 | 64 | 69.130 ms | 242.69 Melem/s |
| 4096x4096 | 128 | 70.353 ms | 238.47 Melem/s |

### Analysis

- **Tile size impact:** Smaller tiles (32) generally provide better throughput, especially at larger image sizes
- **Scaling:** Performance scales roughly linearly with image area for larger images
- **Regression noted:** Several benchmarks show 5-15% regression compared to previous runs, potentially due to environmental factors or code changes

### SIMD vs Scalar Performance

| Operation | Size | Scalar | SIMD | Speedup |
|-----------|------|--------|------|---------|
| sum_and_sum_sq | 4096 | 3.47 µs | 427.73 ns | **8.1x** |
| sum_abs_dev | 4096 | 3.51 µs | 429.98 ns | **8.2x** |
| sum_and_sum_sq | 16384 | 18.81 µs | 2.70 µs | **7.0x** |
| sum_abs_dev | 16384 | 18.97 µs | 2.76 µs | **6.9x** |
| sum_and_sum_sq | 65536 | 42.24 µs | 5.40 µs | **7.8x** |
| sum_abs_dev | 65536 | 43.03 µs | 5.52 µs | **7.8x** |

**Key finding:** SIMD optimizations provide **7-8x speedup** for background statistics computation.

### Interpolation SIMD vs Scalar

| Size | Scalar | SIMD | Speedup |
|------|--------|------|---------|
| 64 | 43.64 ns | 9.45 ns | **4.6x** |
| 256 | 168.37 ns | 26.98 ns | **6.2x** |
| 1024 | 641.48 ns | 100.35 ns | **6.4x** |
| 4096 | 2.55 µs | 397.87 ns | **6.4x** |

**Key finding:** SIMD interpolation achieves **4.6-6.4x speedup**, with better gains at larger sizes.

---

## 2. Centroid Computation

### Summary Table

| Operation | Size | Time (median) | Throughput |
|-----------|------|---------------|------------|
| refine_centroid | 21x21 | 1.23 µs | - |
| compute_metrics | 21x21 | 1.16 µs | - |
| compute_centroid_batch | 10 stars | 99.88 µs | 100.12 Kelem/s |
| compute_centroid_batch | 100 stars | 1.12 ms | 89.03 Kelem/s |
| compute_centroid_batch | 500 stars | 6.55 ms | 76.38 Kelem/s |

### Analysis

- **Per-star overhead:** ~10 µs per star for centroid computation
- **Scaling:** Throughput decreases slightly with batch size, likely due to cache effects
- **Batch performance regressed** by 14% for 500-star batches

### Gaussian Fit Performance

| Window Size | Time (median) |
|-------------|---------------|
| 15x15 | 12.14 µs |
| 21x21 | 106.11 µs |
| 31x31 | 263.78 µs |

### Moffat Fit Performance

| Window Size | Fixed Beta | Variable Beta |
|-------------|------------|---------------|
| 15x15 | 21.17 µs | 98.56 µs |
| 21x21 | 40.39 µs | 65.20 µs |
| 31x31 | 102.03 µs | 314.40 µs |

**Key finding:** Variable beta Moffat fitting is **3-5x slower** than fixed beta, which is expected due to additional parameter optimization.

---

## 3. Convolution

### Gaussian Convolution Performance

| Image Size | Sigma | Time (median) | Throughput |
|------------|-------|---------------|------------|
| 256x256 | 1 | 160.02 µs | 409.54 Melem/s |
| 256x256 | 2 | 243.00 µs | 269.69 Melem/s |
| 256x256 | 3 | 328.41 µs | 199.55 Melem/s |
| 512x512 | 1 | 502.70 µs | 521.48 Melem/s |
| 512x512 | 2 | 816.40 µs | 321.10 Melem/s |
| 512x512 | 3 | 1.17 ms | 224.66 Melem/s |
| 1024x1024 | 1 | 5.84 ms | 179.58 Melem/s |
| 1024x1024 | 2 | 6.70 ms | 156.49 Melem/s |
| 1024x1024 | 3 | 8.27 ms | 126.78 Melem/s |
| 2048x2048 | 1 | 15.05 ms | 278.62 Melem/s |
| 2048x2048 | 2 | 21.24 ms | 197.45 Melem/s |
| 2048x2048 | 3 | 24.48 ms | 171.34 Melem/s |

### Analysis

- **Sigma impact:** Higher sigma values increase kernel size, reducing throughput by ~30-40% per sigma increment
- **Scaling:** Throughput varies with image size due to cache effects; mid-range sizes (512x512) often have highest throughput

### Matched Filter Performance

| Image Size | FWHM | Time (median) | Throughput |
|------------|------|---------------|------------|
| 512x512 | 3 | 958.31 µs | 273.55 Melem/s |
| 512x512 | 4 | 1.30 ms | 201.90 Melem/s |
| 512x512 | 5 | 1.44 ms | 181.87 Melem/s |
| 1024x1024 | 3 | 4.11 ms | 255.25 Melem/s |
| 1024x1024 | 4 | 6.13 ms | 171.00 Melem/s |
| 1024x1024 | 5 | 7.25 ms | 144.61 Melem/s |
| 2048x2048 | 3 | 21.95 ms | 191.07 Melem/s |
| 2048x2048 | 4 | 24.57 ms | 170.72 Melem/s |
| 2048x2048 | 5 | 26.08 ms | 160.81 Melem/s |

### SIMD vs Scalar Row Convolution

| Row Size | Scalar | SIMD | Speedup |
|----------|--------|------|---------|
| 256 | 3.42 µs | 263.92 ns | **13.0x** |
| 512 | 6.52 µs | ~500 ns (est.) | **~13x** |

**Key finding:** SIMD row convolution provides **~13x speedup**, making it one of the most effective SIMD optimizations.

---

## 4. Cosmic Ray Detection

### Laplacian Computation

| Image Size | Time (median) | Throughput |
|------------|---------------|------------|
| 512x512 | 63.84 µs | 4.11 Gelem/s |
| 1024x1024 | 303.02 µs | 3.46 Gelem/s |
| 2048x2048 | 1.90 ms | 2.21 Gelem/s |

### SIMD vs Scalar Cosmic Ray Detection

| Image Size | Scalar | SIMD | Speedup |
|------------|--------|------|---------|
| 512x512 | 502.62 µs | 55.95 µs | **9.0x** |
| 1024x1024 | 4.18 ms | 480.04 µs | **8.7x** |
| 2048x2048 | 16.68 ms | 3.23 ms | **5.2x** |

**Key finding:** SIMD cosmic ray detection provides **5-9x speedup**, with higher gains at smaller image sizes.

### Full Cosmic Ray Detection Pipeline

| Image Size | Time (median) | Throughput |
|------------|---------------|------------|
| 512x512 | 8.10 ms | 32.37 Melem/s |
| 1024x1024 | 33.95 ms | 30.88 Melem/s |

---

## 5. Detection & Thresholding

### Threshold Mask Creation

| Image Size | Time (median) | Throughput |
|------------|---------------|------------|
| 512x512 | 44.85 µs | 5.85 Gelem/s |
| 1024x1024 | 223.51 µs | 4.69 Gelem/s |
| 2048x2048 | 1.70 ms | 2.46 Gelem/s |

**Key finding:** Threshold mask creation is extremely fast, achieving **2.5-5.8 Gelem/s throughput**.

### Mask Dilation

| Image Size | Radius | Time (median) | Throughput |
|------------|--------|---------------|------------|
| 512x512 | 1 | 172.02 µs | 1.52 Gelem/s |
| 512x512 | 2 | 233.24 µs | 1.12 Gelem/s |
| 512x512 | 3 | 323.26 µs | 810.94 Melem/s |
| 1024x1024 | 1 | 529.42 µs | 1.98 Gelem/s |
| 1024x1024 | 2 | 591.13 µs | 1.77 Gelem/s |
| 1024x1024 | 3 | 668.05 µs | 1.57 Gelem/s |
| 2048x2048 | 1 | 1.92 ms | 2.19 Gelem/s |
| 2048x2048 | 2 | 1.97 ms | 2.12 Gelem/s |
| 2048x2048 | 3 | 2.07 ms | 2.03 Gelem/s |

**Key finding:** Dilation radius has moderate impact; radius 3 is ~1.9x slower than radius 1 for small images but only ~1.1x slower for large images due to memory bandwidth limitations.

---

## 6. Deblending

### Local Maxima Detection

| Configuration | Time (median) | Throughput |
|---------------|---------------|------------|
| 10 pairs, sep=15 | 45.34 µs | 220.54 Kelem/s |
| 50 pairs, sep=12 | 138.68 µs | 360.53 Kelem/s |
| 100 pairs, sep=10 | 343.55 µs | 291.08 Kelem/s |

### Multi-threshold Deblending

| Configuration | Time (median) | Throughput |
|---------------|---------------|------------|
| 10 pairs, sep=15 | 25.02 ms | 399.64 elem/s |
| 50 pairs, sep=12 | 108.25 ms | 461.91 elem/s |

**Key finding:** Multi-threshold deblending is computationally expensive at **~2 ms per star pair**.

---

## Summary of Key Optimizations

| Component | SIMD Speedup | Notes |
|-----------|--------------|-------|
| Background sum_and_sum_sq | 7-8x | Consistent across sizes |
| Background interpolation | 4.6-6.4x | Better at larger sizes |
| Row convolution | ~13x | Most effective optimization |
| Cosmic ray detection | 5-9x | Better at smaller sizes |

## Recommendations

1. **Row convolution SIMD** is highly effective and should be ensured for all convolution operations
2. **Background estimation** could benefit from parallel processing across tiles for larger images
3. **Multi-threshold deblending** is a bottleneck; consider optimizations for crowded fields
4. **Memory bandwidth** becomes limiting for 2K+ images; consider tiled processing
5. **Benchmark variability** is high (10-15% outliers common); use more samples for precise measurements

## Performance Targets

For real-time processing of a 4K x 4K image:
- Background estimation: ~70 ms (current)
- Gaussian convolution (sigma=2): ~80-100 ms (estimated)
- Full star detection pipeline: ~200-300 ms target

Current performance is suitable for interactive use but not real-time video processing.
