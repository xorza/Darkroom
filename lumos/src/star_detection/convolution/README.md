# Convolution Module

Gaussian convolution for matched filtering in star detection. This module implements separable Gaussian convolution used by algorithms like DAOFIND and SExtractor to boost SNR for faint star detection.

## Current Implementation

- **Separable Convolution**: O(n×k) instead of O(n×k²) for circular Gaussians
- **SIMD Acceleration**: AVX2/SSE on x86_64, NEON on aarch64
- **Parallel Processing**: Row/column passes parallelized with rayon
- **Adaptive Strategy**: Automatic selection between SIMD-transpose and scalar column convolution based on image size threshold (4M pixels)
- **Mirror Boundary Handling**: Reflects pixels at image edges

## Optimization Best Practices

### 1. Separable Kernels

For separable kernels (Gaussian, box filters), decompose 2D convolution into two 1D passes:
- Complexity drops from O(n×k²) to O(n×2k) where k is kernel size
- Our implementation uses this for circular Gaussians
- Elliptical Gaussians require full 2D convolution (not separable)

### 2. SIMD Vectorization

Current SIMD optimizations:
- Process 8 pixels per iteration (AVX2) or 4 pixels (SSE/NEON)
- Aligned memory access where possible
- Horizontal sum via store+scalar (faster than `hadd` chains)

Further SIMD improvements:
- **Prefetching**: Use `_mm_prefetch` for upcoming rows
- **FMA**: Use `_mm256_fmadd_ps` on supported CPUs (requires FMA feature detection)
- **Kernel broadcasting**: Pre-broadcast kernel values to SIMD registers outside inner loop
- **Unrolled loops**: 2x or 4x unrolling can hide latency

### 3. Cache-Friendly Access Patterns

Current optimizations:
- Block transposition with 64×64 blocks for column convolution
- Row-major processing to maximize L1/L2 cache hits

Additional strategies:
- **Strip mining**: Process vertical strips that fit in L2 cache
- **Loop tiling**: Tile both passes to keep working set in cache
- **Prefetch distance tuning**: Adjust prefetch distance based on memory latency

### 4. FFT vs Direct Convolution

**When to use FFT (O(n log n)):**
- Kernel size > ~15-20 for single precision
- Fixed kernel applied to many images
- Frequency domain operations needed anyway

**When to use direct (O(n×k)):**
- Small kernels (k < 15)
- Variable kernel sizes
- Memory-constrained environments (FFT needs complex buffers)

**Hybrid approaches:**
- Overlap-add/overlap-save for streaming data
- Winograd transforms for small fixed kernels

### 5. Parallel Strategies

Current implementation:
- Row-parallel processing with rayon
- Automatic chunk sizing via `par_chunks_auto_aligned`

Advanced parallelism:
- **NUMA awareness**: Pin threads to local memory nodes for large images
- **Work stealing**: Already provided by rayon
- **Pipeline parallelism**: Overlap I/O with computation for streaming

### 6. Gaussian Approximations

For approximate Gaussian blur (if exact convolution not required):

| Method | Complexity | Quality | Notes |
|--------|------------|---------|-------|
| Box blur (3-pass) | O(n) | ~97% | Sum area table or sliding window |
| Stack blur | O(n) | ~95% | Single pass, good for real-time |
| IIR recursive | O(n) | ~99% | Deriche/van Vliet filters |
| Binomial filter | O(n×k) | Exact at integer σ | Limited to σ = √(k/4) |

**IIR Recursive Filters (Deriche/van Vliet):**
- O(n) regardless of sigma
- Forward-backward pass for zero phase
- Coefficients precomputed from sigma
- Caution: numerical stability at large sigma

### 7. GPU Convolution

For batch processing or real-time applications:

**Approaches:**
- Texture memory with hardware interpolation
- Shared memory tiling for compute shaders
- Separable passes with intermediate texture

**Considerations:**
- Memory transfer overhead (CPU→GPU→CPU)
- Occupancy optimization
- Warp divergence at boundaries

### 8. Boundary Handling

Current: Mirror reflection (`mirror_index`)

Options by cost:
1. **Zero padding**: Fastest, but darkens edges
2. **Clamp/replicate**: Fast, slight edge artifacts
3. **Mirror/reflect**: Current choice, good quality
4. **Wrap**: For periodic signals only
5. **Custom**: Pre-pad buffer to avoid branch in inner loop

**Pre-padding optimization:**
```
Pad input buffer by kernel radius, then inner loop needs no boundary checks.
Memory overhead: 2×radius×(width+height) floats
Benefit: Branchless inner loop, better SIMD utilization
```

## Benchmarking Notes

Key benchmarks to run:
```bash
cargo test -p lumos --release bench_convolve -- --ignored --nocapture
```

Performance factors to measure:
- Varying image sizes (1K×1K to 16K×16K)
- Varying kernel sizes (σ = 1.0 to 5.0)
- SIMD vs scalar throughput
- Transpose threshold validation (currently 4M pixels)

## Future Optimization Opportunities

1. **Pre-padded buffers**: Eliminate boundary checks in inner loop
2. **FMA intrinsics**: Fused multiply-add for ~2x throughput
3. **Streaming SIMD**: Process multiple rows simultaneously
4. **IIR filters**: O(n) alternative for large sigma values
5. **GPU offload**: For batch processing pipelines
6. **AVX-512**: 16 floats per iteration on supported hardware

## References

- DAOFIND algorithm (Stetson 1987)
- SExtractor (Bertin & Arnouts 1996)
- Deriche recursive Gaussian filter
- Intel Intrinsics Guide for SIMD optimization
- "Image Processing on GPU" (GPU Gems)
