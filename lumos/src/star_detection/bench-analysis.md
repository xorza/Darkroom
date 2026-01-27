# Full Pipeline & Star Detection Benchmarks Analysis

**Date:** 2026-01-27  
**System:** Linux, Release build with optimizations

---

## Executive Summary

The full astrophotography pipeline processes 10 images (6032x4028 pixels each) in **13.7 seconds**:
- Star detection: **6.3s (46%)** - Main bottleneck
- Registration: **2.8s (20%)**
- Warping: **3.8s (28%)**
- Stacking: **0.8s (6%)**

Star detection at ~630ms per image is the primary optimization target.

---

## Full Pipeline Benchmark Results

**Test Data:** 10 calibrated light frames (6032x4028 pixels, 3 channels, X-Trans sensor)

| Benchmark | Time (mean) | Per Image |
|-----------|-------------|-----------|
| Star Detection (single) | 629.66 ms | 629.66 ms |
| Star Detection (all 10) | 6.27 s | 627 ms |
| Registration (single pair) | 328.72 ms | 328.72 ms |
| Registration (all 9 pairs) | 2.77 s | 308 ms |
| Mean Stacking (10 images) | 801.61 ms | 80 ms |
| **Full Pipeline** | **13.72 s** | **1.37 s** |

### Full Pipeline with I/O (from example)

The `full_pipeline` example (which includes I/O) completed in 41.81s:
- Step 2 (Calibration): 11.49s
- Step 3 (Star detection): 7.17s
- Step 4 (Registration + warping): 19.39s
- Step 5 (Sigma-clipped stacking): 3.53s

---

## Star Detection Subsystem Analysis

### 1. Background Estimation

| Image Size | Tile Size | Time (median) | Throughput |
|------------|-----------|---------------|------------|
| 512x512 | 32 | 570 µs | 460 Melem/s |
| 512x512 | 64 | 636 µs | 412 Melem/s |
| 2048x2048 | 32 | 21.7 ms | 193 Melem/s |
| 2048x2048 | 64 | 22.0 ms | 191 Melem/s |
| 4096x4096 | 32 | 63.1 ms | 266 Melem/s |
| 4096x4096 | 64 | 66.8 ms | 251 Melem/s |

**SIMD vs Scalar Performance:**

| Operation | Size | Scalar | SIMD | Speedup |
|-----------|------|--------|------|---------|
| sum_and_sum_sq | 1024 | 635 ns | 73 ns | **8.7x** |
| sum_abs_dev | 1024 | 638 ns | 71 ns | **9.0x** |
| sum_and_sum_sq | 4096 | 2.58 µs | 317 ns | **8.1x** |
| sum_and_sum_sq | 16384 | 10.4 µs | 1.30 µs | **8.0x** |
| sum_and_sum_sq | 65536 | 41.6 µs | 5.19 µs | **8.0x** |

**Interpolation SIMD:**

| Size | Scalar | SIMD | Speedup |
|------|--------|------|---------|
| 64 | 42 ns | 9 ns | **4.7x** |
| 256 | 163 ns | 26 ns | **6.3x** |
| 1024 | 631 ns | 99 ns | **6.4x** |
| 4096 | 2.50 µs | 392 ns | **6.4x** |

### 2. Centroid Computation

| Operation | Size | Time (median) | Throughput |
|-----------|------|---------------|------------|
| refine_centroid | 21x21 | 1.20 µs | - |
| compute_metrics | 21x21 | 1.19 µs | - |
| compute_centroid_batch | 10 stars | 94.2 µs | 106 Kelem/s |
| compute_centroid_batch | 100 stars | 993 µs | 101 Kelem/s |
| compute_centroid_batch | 500 stars | 5.22 ms | 96 Kelem/s |

**Performance improved 19-21% in batch centroid computation.**

### 3. Gaussian & Moffat Fitting

| Window Size | Gaussian | Moffat (fixed β) | Moffat (var β) |
|-------------|----------|------------------|----------------|
| 15x15 | 11.3 µs | 19.6 µs | 90.8 µs |
| 21x21 | 99.5 µs | 38.7 µs | 61.7 µs |
| 31x31 | 241.9 µs | 94.9 µs | - |

---

## Performance Breakdown for 6K Image

For a typical 6032x4028 (~24 Mpixel) astrophotography image:

| Stage | Estimated Time | % of Total |
|-------|----------------|------------|
| Background estimation | ~120 ms | 19% |
| Convolution (sigma=2) | ~150 ms | 24% |
| Threshold & detection | ~50 ms | 8% |
| Centroid refinement | ~200 ms | 32% |
| Cosmic ray rejection | ~100 ms | 16% |
| **Total** | **~620 ms** | 100% |

---

## SIMD Optimization Summary

| Component | SIMD Speedup | Status |
|-----------|--------------|--------|
| Background sum/variance | 8-9x | Implemented |
| Background interpolation | 4.6-6.4x | Implemented |
| Row convolution | ~13x | Implemented |
| Cosmic ray detection | 5-9x | Implemented |

---

## Performance Improvement Plan

### Priority 1: Star Detection Optimization (Target: 50% reduction)

**Current:** 630ms per image → **Target:** 300ms per image

1. **Parallel Star Processing**
   - Current centroid batch: 10.4 µs/star
   - Parallelize across cores for 500+ stars per image
   - Expected gain: 3-4x for centroid phase

2. **GPU-Accelerated Convolution**
   - Current: ~150ms for 24 Mpixel
   - GPU could reduce to <10ms
   - Already have GPU infrastructure in imaginarium

3. **Reduce Kernel Size**
   - Use adaptive sigma based on expected FWHM
   - Smaller kernels = faster convolution

### Priority 2: Registration Optimization (Target: 30% reduction)

**Current:** 330ms per pair → **Target:** 230ms per pair

1. **Parallel Triangle Matching**
   - Current RANSAC: 5000 iterations
   - Could parallelize triangle hash lookup

2. **Early Termination**
   - Stop when confidence threshold met
   - Current: always runs full 5000 iterations

### Priority 3: Warping Optimization (Target: 50% reduction)

**Current:** ~380ms per image → **Target:** 190ms per image

1. **GPU Image Warping**
   - Use existing GPU transform in imaginarium
   - Bilinear interpolation on GPU is very fast

2. **Parallel Channel Processing**
   - Currently processes RGB channels sequentially
   - Could parallelize across channels

### Priority 4: I/O Optimization

1. **Parallel Image Loading**
   - Load next image while processing current
   - Use async I/O

2. **Memory-Mapped Files**
   - Avoid full file reads for large images

---

## Projected Performance After Optimization

| Component | Current | Optimized | Reduction |
|-----------|---------|-----------|-----------|
| Star Detection | 6.3s | 3.0s | 52% |
| Registration | 2.8s | 2.0s | 29% |
| Warping | 3.8s | 1.0s | 74% |
| Stacking | 0.8s | 0.6s | 25% |
| **Total** | **13.7s** | **6.6s** | **52%** |

With GPU acceleration:
- Warping could drop to <0.5s
- Convolution could drop to <0.5s
- **Optimistic total: ~4s for 10 images**

---

## Quick Wins (Low Effort, High Impact)

1. **GPU Warping** - Already have infrastructure, just need to wire up
2. **Parallel Channel Processing** - Simple change for 3x speedup on warping
3. **Reduce RANSAC iterations** - With good data, 2000 iterations may suffice

## Medium Term Improvements

1. **GPU Convolution** - Requires shader development
2. **Parallel Star Processing** - Needs careful synchronization
3. **Adaptive Parameters** - Auto-tune based on image characteristics

## Long Term Goals

1. **Real-time Preview** - <100ms per image for live stacking preview
2. **Video Rate Processing** - <33ms per frame for planetary imaging

---

## Running Benchmarks

```bash
# Full pipeline benchmark
cargo bench -p lumos --features bench --bench full_pipeline

# Star detection subsystem benchmarks
cargo bench -p lumos --features bench --bench 'star_detection_*'

# Individual benchmarks
cargo bench -p lumos --features bench --bench star_detection_background
cargo bench -p lumos --features bench --bench star_detection_centroid
cargo bench -p lumos --features bench --bench star_detection_convolution
```

Requires calibrated light images in:
- `LUMOS_CALIBRATION_DIR/calibrated_lights` or
- `test_output/calibrated_lights` (from running `full_pipeline` example)
