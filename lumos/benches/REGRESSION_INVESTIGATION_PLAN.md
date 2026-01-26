# Benchmark Regression Investigation Plan

Generated: 2026-01-26

## Overview

Several benchmarks show significant performance regressions that need investigation. This document outlines the investigation plan for each affected benchmark.

---

## 1. Gaussian Convolution (CRITICAL)

**Location:** `star_detection_convolution.rs`

**Regression Severity:** HIGH (32% to 95% slower)

| Size | Sigma | Regression |
|------|-------|------------|
| 256x256 | 1 | +36% |
| 256x256 | 2 | +95% |
| 256x256 | 3 | +51% |
| 512x512 | 1 | +83% |
| 512x512 | 2 | +85% |
| 512x512 | 3 | +79% |
| 1024x1024 | 1 | +48% |
| 1024x1024 | 2 | +58% |
| 1024x1024 | 3 | +69% |
| 2048x2048 | 1 | +48% |
| 2048x2048 | 2 | +42% |
| 2048x2048 | 3 | +55% |

### Investigation Steps

- [ ] Check recent commits affecting `gaussian_convolve` or related convolution code
- [ ] Profile with `perf` or `flamegraph` to identify hotspots
- [ ] Compare assembly output before/after regression (use `cargo-show-asm`)
- [ ] Check if SIMD intrinsics are still being used correctly
- [ ] Verify compiler optimizations haven't changed (check `rustc` version)
- [ ] Look for memory allocation changes (extra allocations per iteration?)
- [ ] Check if kernel computation changed (sigma_2 has worst regression)
- [ ] Verify separable convolution is still being used (horizontal + vertical pass)

### Possible Causes

1. SIMD code path disabled or broken
2. Extra memory allocations introduced
3. Loop unrolling or vectorization regressed
4. Kernel size computation changed
5. Cache behavior degraded

---

## 2. Matched Filter (CRITICAL)

**Location:** `star_detection_convolution.rs`

**Regression Severity:** HIGH (20% to 94% slower)

| Size | FWHM | Regression |
|------|------|------------|
| 512x512 | 3 | +84% |
| 512x512 | 4 | +83% |
| 512x512 | 5 | +35% |
| 1024x1024 | 3 | +40% |
| 1024x1024 | 4 | +27% |
| 1024x1024 | 5 | +26% |
| 2048x2048 | 3 | +24% |
| 2048x2048 | 4 | No change |
| 2048x2048 | 5 | No change |

### Investigation Steps

- [ ] Check if matched_filter uses gaussian_convolve internally (likely related regression)
- [ ] Profile to see if convolution is the bottleneck
- [ ] Check kernel generation for different FWHM values
- [ ] Verify FWHM 4-5 at 2048x2048 to understand why they're stable
- [ ] Compare memory access patterns across sizes

### Possible Causes

1. Directly related to gaussian_convolve regression
2. Kernel size differences for different FWHM values
3. Cache pressure at smaller sizes

---

## 3. Cosmic Ray SIMD (HIGH)

**Location:** `star_detection_cosmic_ray.rs`

**Regression Severity:** HIGH (size-dependent)

| Size | Scalar Change | SIMD Change |
|------|---------------|-------------|
| 512x512 | +0.6% (stable) | +0.4% (stable) |
| 1024x1024 | +3.6% | **+146%** |
| 2048x2048 | +2.6% | **+60%** |

### Investigation Steps

- [ ] Check SIMD implementation for size-dependent code paths
- [ ] Look for buffer size thresholds that trigger different behavior
- [ ] Profile SIMD path at 1024x1024 specifically
- [ ] Check for alignment issues at specific sizes
- [ ] Verify SIMD registers aren't being spilled at larger sizes
- [ ] Look for loop tiling or blocking that changes at 1024x1024
- [ ] Check if there's a fallback to scalar code at certain sizes

### Possible Causes

1. SIMD implementation has a size threshold bug
2. Memory alignment issues at specific sizes
3. Cache line conflicts at 1024x1024
4. Incorrect SIMD loop bounds calculation

---

## 4. Convolution Row SIMD (MEDIUM)

**Location:** `star_detection_convolution.rs`

**Regression Severity:** MEDIUM (4% to 11% slower)

| Size | Scalar Change | SIMD Change |
|------|---------------|-------------|
| 256 | +6% | +5% |
| 512 | +11% | +4% |
| 1024 | +7% | stable |
| 2048 | stable | stable |

### Investigation Steps

- [ ] Check if this is related to the gaussian_convolve regression
- [ ] Profile row convolution separately
- [ ] Verify SIMD intrinsics are being used
- [ ] Check for changes in kernel handling

### Possible Causes

1. Related to overall convolution regression
2. Small overhead added to row processing

---

## 5. Scale Operation (LOW)

**Location:** `math.rs`

**Regression Severity:** LOW (9% to 11% slower)

| Size | Change |
|------|--------|
| 64 | -6.5% (improved) |
| 256 | **+10%** |
| 1024 | +3% (noise) |
| 4096 | **+11%** |

### Investigation Steps

- [ ] Check if vectorization changed for specific sizes
- [ ] Profile to see where time is spent
- [ ] Verify SIMD code paths

### Possible Causes

1. Compiler optimization changes
2. Memory alignment at specific sizes

---

## 6. Median Filter 3x3 (LOW)

**Location:** `median_filter.rs`

**Regression Severity:** LOW (10% slower at 1024x1024 only)

| Size | Change |
|------|--------|
| 512x512 | -12% (improved) |
| 1024x1024 | **+10%** |
| 4096x4096 | stable |

### Investigation Steps

- [ ] Check for size-dependent code paths
- [ ] Profile at 1024x1024 specifically
- [ ] Check cache behavior

### Possible Causes

1. Cache pressure at specific size
2. Thread scheduling variance

---

## 7. Background Estimation (LOW)

**Location:** `star_detection_background.rs`

**Regression Severity:** LOW (3% slower at 4096x4096)

| Size | Tile | Change |
|------|------|--------|
| 512x512 | 32 | -32% (improved) |
| 512x512 | 64 | -29% (improved) |
| 4096x4096 | 32 | **+3%** |
| 4096x4096 | 64 | **+3%** |
| 4096x4096 | 128 | +2% |

### Investigation Steps

- [ ] This may be noise - verify with multiple runs
- [ ] Check memory allocation at large sizes
- [ ] Profile to identify any hotspots

### Possible Causes

1. Likely measurement noise
2. Minor memory allocation overhead

---

## 8. Dilate Mask (LOW)

**Location:** `star_detection_detection.rs`

**Regression Severity:** LOW (3% to 12% slower)

| Size | Radius | Change |
|------|--------|--------|
| 512x512 | 1 | +1% (noise) |
| 512x512 | 2 | **+12%** |
| 512x512 | 3 | **+8%** |
| 1024x1024 | 2 | +4% |
| 2048x2048 | 2 | +4% |

### Investigation Steps

- [ ] Check radius-dependent code paths
- [ ] Profile dilation at radius 2 and 3
- [ ] Verify structuring element generation

### Possible Causes

1. Changes in structuring element handling
2. Loop bound changes for different radii

---

## Priority Order

1. **gaussian_convolve** - Most severe, affects multiple benchmarks
2. **matched_filter** - Likely related to #1
3. **cosmic_ray SIMD** - Clear bug in size-dependent code
4. **convolution_row** - May resolve with #1
5. **dilate_mask** - Independent, lower priority
6. **scale** - Low priority
7. **median_filter** - Low priority, may be noise
8. **background_estimation** - Likely noise

---

## Investigation Tools

```bash
# Profile with perf
perf record -g cargo bench -p lumos --features bench --bench star_detection_convolution -- gaussian_convolve
perf report

# Generate flamegraph
cargo flamegraph --bench star_detection_convolution -- --bench gaussian_convolve

# Check assembly
cargo asm -p lumos --lib lumos::convolution::gaussian_convolve

# Compare with baseline
cargo bench -p lumos --features bench --bench star_detection_convolution -- --save-baseline before
# ... make changes ...
cargo bench -p lumos --features bench --bench star_detection_convolution -- --baseline before

# Run specific benchmark multiple times
for i in {1..5}; do cargo bench -p lumos --features bench --bench star_detection_convolution -- gaussian_convolve/512x512/sigma_2; done
```

---

## Notes

- All regressions should be bisected using `git bisect` if recent commits are suspected
- Consider running benchmarks on isolated CPU cores for more stable results
- Check `criterion` history in `target/criterion/` for trend data
