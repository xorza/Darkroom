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

### 4. Detection (Threshold Mask)

```bash
cargo bench -p lumos --features bench --bench star_detection_detection
```

**SIMD vs Scalar benchmarks:**

| Benchmark Group | Operation | Sizes |
|-----------------|-----------|-------|
| `threshold_mask_simd_vs_scalar` | `create_threshold_mask` | 512x512, 1024x1024, 2048x2048 |

**Functions compared:**
- `create_threshold_mask_simd()` vs `create_threshold_mask()` (scalar)

### 5. Median Filter

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

# Detection SIMD comparison
cargo bench -p lumos --features bench --bench star_detection_detection -- simd_vs_scalar

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

## Expected Speedups

Based on typical SIMD implementations:

| Operation | Expected NEON Speedup |
|-----------|----------------------|
| Sum/Sum-squared | 3-4x |
| Sum absolute deviations | 3-4x |
| Interpolation | 2-3x |
| Row convolution | 2-4x |
| Laplacian | 5-6x |
| Threshold mask | 3-4x |
| Median filter (sorting network) | 2x |

## Platform Support

- **aarch64 (Apple Silicon)**: NEON SIMD
- **x86_64**: SSE2/SSE4.1/AVX2 SIMD
- **Other**: Scalar fallback

The benchmarks automatically use the best available SIMD implementation for the current platform.
