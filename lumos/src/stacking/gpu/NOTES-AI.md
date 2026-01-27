# GPU Stacking Module - NOTES-AI

**Last Updated**: 2026-01-27

## Overview

This module provides GPU-accelerated stacking operations using wgpu compute shaders.

## Components

### 1. GpuSigmaClipper (`sigma_clip.rs`)

Single-batch GPU sigma clipping for up to 128 frames.

**Key Types**:
- `GpuSigmaClipConfig`: Configuration (sigma threshold, max iterations)
- `GpuSigmaClipper`: Main API for GPU sigma clipping
- `MAX_GPU_FRAMES`: 128 (shader limit)

**Algorithm**:
1. Upload all frame data to GPU storage buffer
2. Compute per-pixel mean and standard deviation
3. Iteratively reject outliers (values > sigma × stddev from mean)
4. Return clipped mean per pixel

### 2. BatchPipeline (`batch_pipeline.rs`)

Multi-batch processing for large stacks (>128 frames) with overlapped compute/transfer.

**Key Types**:
- `BatchPipelineConfig`: Configuration (sigma clip settings, batch size, buffer count)
- `BatchPipeline`: Main API for batched GPU stacking
- `BufferSlot`: Double/triple buffer management
- `PendingReadback`: Async readback tracking

**Features**:
- Handles >128 frames by splitting into batches
- Double-buffering for overlapped operations
- Async GPU operations with `map_async` callbacks
- Weighted mean combination of batch results

### 3. GpuSigmaClipPipeline (`pipeline.rs`)

Low-level wgpu compute pipeline setup.

**Shader**: `sigma_clip.wgsl`
- 16×16 workgroup size
- Per-pixel iterative sigma clipping
- In-place sorting for statistics

## Benchmark Results

See `benches/stacking/bench-analysis.md` for detailed analysis.

**Summary**:
- GPU sigma clipping: 5-18% faster than CPU for large stacks (>50 frames)
- CPU is faster for small stacks (<30 frames) due to GPU overhead
- Batch pipeline maintains ~240-280 Melem/s throughput
- Async mode provides infrastructure but no benefit for in-memory data

**Recommendations**:
- Default to CPU for typical astrophotography (<50 frames)
- Use GPU for very large stacks (>50 frames)
- Async mode benefits I/O-bound workloads (loading from disk)

## Limitations

1. **Buffer size**: wgpu max buffer is 1GB, limiting single-batch capacity
2. **Frame count**: Shader supports max 128 frames per batch
3. **Memory**: All frames must fit in GPU memory for optimal performance

## Usage

```rust
// Single batch (≤128 frames)
let mut clipper = GpuSigmaClipper::new();
let result = clipper.stack(&frames, width, height, &config);

// Large stacks (>128 frames)
let mut pipeline = BatchPipeline::new(BatchPipelineConfig::default());
let result = pipeline.stack(&frames, width, height);

// Async for I/O overlap (>2 batches)
let result = pipeline.stack_async(&frames, width, height);
```

## File Structure

```
src/stacking/gpu/
├── mod.rs              # Module exports
├── sigma_clip.rs       # GpuSigmaClipper API
├── sigma_clip.wgsl     # Compute shader
├── pipeline.rs         # wgpu pipeline setup
├── batch_pipeline.rs   # BatchPipeline for large stacks
├── bench.rs            # Benchmark functions
└── NOTES-AI.md         # This file
```
