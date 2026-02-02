# Convolution Module

Gaussian convolution for matched filtering in star detection. This module implements separable Gaussian convolution used by algorithms like DAOFIND and SExtractor to boost SNR for faint star detection.

## Performance

| Benchmark | Time | Notes |
|-----------|------|-------|
| Gaussian 1K | 1.8ms | Separable convolution |
| Gaussian 4K | 23.8ms | Memory bandwidth limited |
| Elliptical 1K | 2.8ms | Full 2D convolution |
| Matched filter 1K circular | 2.8ms | With background subtraction |
| Matched filter 1K elliptical | 3.3ms | With background subtraction |
| Matched filter 4K | 33.5ms | With background subtraction |

## Implementation

### Separable Convolution (Circular Gaussian)
- O(n×k) instead of O(n×k²) where k is kernel size
- Separate row and column passes
- Column pass uses row-major SIMD processing for cache locality

### SIMD Acceleration
- **Row convolution**: Process 8 pixels (AVX2), 4 pixels (SSE/NEON) per iteration
- **Column convolution**: Row-major processing, loads contiguous memory for all kernel rows per output pixel
- **2D convolution**: Per-row SIMD with parallel dispatch via rayon
- FMA intrinsics for fused multiply-add
- Horizontal sum via store+scalar (faster than `hadd` chains)

### Parallel Processing
- Row/column passes parallelized with rayon
- 2D elliptical convolution uses parallel row dispatch

### Boundary Handling
- Mirror reflection at image edges
- Scalar fallback at boundaries where SIMD width exceeds available pixels

## Architecture

### Files
- `mod.rs`: Main convolution logic, separable and 2D convolution
- `simd/mod.rs`: SIMD dispatch functions with runtime CPU feature detection
- `simd/sse.rs`: AVX2 and SSE4.1 implementations
- `simd/neon.rs`: ARM NEON implementations

### Key Functions
- `gaussian_convolve`: Separable convolution for circular Gaussian
- `elliptical_gaussian_convolve`: Full 2D convolution for elliptical Gaussian
- `matched_filter_convolve`: Convolution with background subtraction

### SIMD Dispatch
- `simd::convolve_row`: Row convolution (AVX2 → SSE → NEON → scalar fallback)
- `simd::convolve_cols_direct`: Column convolution with row-major access
- `simd::convolve_2d_row`: Per-row 2D convolution for elliptical kernels

## Design Decisions

### Direct SIMD Column Convolution vs Transpose
Initially tested transpose-based column convolution (transpose → row SIMD → transpose back).

**Result**: Direct SIMD was faster.
- Transpose overhead exceeded SIMD benefits
- Two full-image memory copies added latency
- 1K images: transpose was 24% slower
- 4K images: transpose was 6% slower

Current implementation uses row-major SIMD processing: iterates output row-by-row, processing 8 columns (AVX2) at a time, loading contiguous memory for each kernel row.

### Separate vs Fused Background Subtraction
Tested fusing background subtraction into convolution to avoid a separate pass.

**Result**: Separate passes were faster.
- Fused approach couldn't use SIMD row convolution (fell back to scalar)
- Matched filter 1K: fused was 36% slower (3.8ms vs 2.8ms)

Current implementation: SIMD background subtraction followed by SIMD convolution.

### Skip Near-Zero Kernel Values
For elliptical 2D convolution, kernel values below 1e-10 are skipped. Sparse kernels (Gaussian tails) benefit from this optimization.

### Scalar Boundary Fallback
Rather than pre-padding buffers to eliminate boundary checks, the implementation uses scalar fallback for edge pixels. Pre-padding would require:
- Additional memory allocation
- Rewriting all SIMD functions
- Marginal benefit (only ~2×radius pixels per row affected)

## Benchmarking

```bash
cargo test -p lumos --release bench_convolve -- --ignored --nocapture
```

## Future Considerations

### IIR Recursive Gaussian
For sigma > 5, IIR filters (Deriche/van Vliet) become O(n) regardless of sigma. Current typical sigma is 1.5-3.0, where direct convolution is competitive.

### AVX-512
Would process 16 floats per iteration but has limited CPU support and can cause frequency throttling.

### GPU Offload
Makes sense for batch processing; transfer overhead too high for single images.

### 4K Performance
Large images are memory-bandwidth limited. Column pass requires strided access patterns that cannot be fully hidden by SIMD.

## References

- DAOFIND algorithm (Stetson 1987)
- SExtractor (Bertin & Arnouts 1996)
- Intel Intrinsics Guide for SIMD optimization
