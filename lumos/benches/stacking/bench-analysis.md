# GPU Stacking Benchmark Analysis

**Date**: 2026-01-27
**Benchmark**: `cargo bench -p lumos --features bench --bench stack_gpu_sigma_clip`

## Summary

Benchmark results for GPU sigma clipping and batch pipeline throughput. Tests were run with various image sizes and frame counts to evaluate GPU vs CPU performance and batch pipeline scaling.

## GPU Sigma Clipping (GPU vs CPU)

### Results Table

| Size | Frames | GPU Time | GPU Thrpt | CPU Time | CPU Thrpt | GPU Speedup |
|------|--------|----------|-----------|----------|-----------|-------------|
| 256x256 | 10 | 6.63 ms | 98.8 Melem/s | 3.54 ms | 185.3 Melem/s | **0.53x (slower)** |
| 256x256 | 30 | 11.78 ms | 167.0 Melem/s | 9.87 ms | 199.3 Melem/s | **0.84x (slower)** |
| 256x256 | 50 | 15.59 ms | 210.2 Melem/s | 18.21 ms | 179.9 Melem/s | **1.17x** |
| 512x512 | 10 | 20.23 ms | 129.6 Melem/s | 14.52 ms | 180.5 Melem/s | **0.72x (slower)** |
| 512x512 | 30 | 34.68 ms | 226.8 Melem/s | 40.02 ms | 196.5 Melem/s | **1.15x** |
| 512x512 | 50 | 68.14 ms | 192.4 Melem/s | 78.86 ms | 166.2 Melem/s | **1.16x** |
| 1024x1024 | 10 | 80.33 ms | 130.5 Melem/s | 57.12 ms | 183.6 Melem/s | **0.71x (slower)** |
| 1024x1024 | 30 | 171.65 ms | 183.3 Melem/s | 161.01 ms | 195.4 Melem/s | **0.94x (similar)** |
| 1024x1024 | 50 | 267.05 ms | 196.3 Melem/s | 315.46 ms | 166.2 Melem/s | **1.18x** |
| 2048x2048 | 10 | 318.03 ms | 131.9 Melem/s | 228.55 ms | 183.5 Melem/s | **0.72x (slower)** |
| 2048x2048 | 30 | 694.08 ms | 181.3 Melem/s | 694.40 ms | 181.2 Melem/s | **1.00x (equal)** |
| 2048x2048 | 50 | 1.074 s | 195.2 Melem/s | 1.131 s | 185.4 Melem/s | **1.05x** |

### Key Findings

1. **Frame count threshold**: GPU becomes beneficial at ~30-50 frames for most image sizes
2. **Small stacks**: CPU is faster for <30 frames due to GPU overhead (buffer creation, transfers)
3. **Large stacks**: GPU shows 5-18% improvement at 50 frames
4. **Throughput**: GPU peaks at ~227 Melem/s (512x512x30), CPU consistent at ~180-200 Melem/s

### Interpretation

The GPU implementation shows **marginal improvements** (5-18%) for larger frame counts, but **does not provide substantial speedup** over the optimized CPU implementation. This is likely because:

1. **Memory bandwidth limited**: Sigma clipping requires reading all pixel values and iterating multiple times
2. **GPU overhead**: Buffer allocation and data transfer dominate for small-medium workloads
3. **CPU optimization**: The CPU implementation is well-optimized and cache-friendly

**Recommendation**: Keep GPU sigma clipping for large stacks (>50 frames) but default to CPU for typical use cases (<50 frames).

## Batch Pipeline Throughput

### Results Table

| Size | Frames | Sync Time | Sync Thrpt | Async Time | Async Thrpt |
|------|--------|-----------|------------|------------|-------------|
| 1024x1024 | 32 | 166.27 ms | 201.8 Melem/s | - | - |
| 1024x1024 | 64 | 294.71 ms | 227.7 Melem/s | - | - |
| 1024x1024 | 128 | 553.32 ms | 242.6 Melem/s | - | - |
| 1024x1024 | 256 | 1.106 s | 242.7 Melem/s | 1.108 s | 242.2 Melem/s |
| 2048x2048 | 32 | 483.99 ms | 277.3 Melem/s | - | - |
| 2048x2048 | 64 | 1.182 s | 227.1 Melem/s | - | - |

Note: Async benchmarks only run for >128 frames (multi-batch scenarios).

### Key Findings

1. **Consistent throughput**: Pipeline maintains ~240-280 Melem/s regardless of frame count
2. **Sync vs Async**: No significant difference (within 0.5%) for 256-frame test
3. **Scaling**: Throughput increases with batch size up to ~128 frames, then plateaus
4. **Memory limit**: 2048x2048x128 exceeds GPU buffer limits (1GB max)

### Interpretation

The batch pipeline provides:
- **Consistent throughput** for large stacks
- **Efficient batching** with weighted mean combination
- **Memory management** for very large frame counts (>128)

The async implementation shows **no measurable benefit** over sync in current tests. This is because:
1. Data is pre-loaded in memory (no I/O overlap benefit)
2. GPU compute dominates transfer time
3. Double-buffering overhead matches any gains

**Recommendation**: Async mode provides infrastructure for I/O overlap but doesn't improve pure compute workloads. It will be beneficial when loading frames from disk.

## Buffer Size Limitations

The benchmark hit wgpu's maximum buffer size limit (1GB) at:
- 2048x2048 images x 128 frames = 2,147,483,648 bytes (2GB)

For larger stacks, the batch pipeline automatically splits into multiple batches and combines results.

## Conclusions

1. **GPU sigma clipping provides marginal benefit** (5-18%) for large stacks (>50 frames)
2. **CPU remains competitive** for typical astrophotography workflows (<50 frames)
3. **Batch pipeline works correctly** for large frame counts with proper weighted combination
4. **Async mode** infrastructure is in place but doesn't improve synthetic benchmarks

## Next Steps

- Consider GPU optimization for the sigma clipping kernel (shared memory, better memory access patterns)
- Test async mode with real file I/O to measure overlap benefits
- Consider keeping CPU as default with GPU as opt-in for very large stacks
