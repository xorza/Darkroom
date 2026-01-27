# GPU Acceleration - Implementation Notes (AI)

## Overview

GPU acceleration for the lumos astrophotography library. Currently uses imaginarium (wgpu-based) for image warping.

## Current State

### GPU Image Warping (IMPLEMENTED)

**File**: `src/registration/gpu.rs`

Uses imaginarium's `Transform` operation with `Affine2` for GPU-accelerated warping.

```rust
GpuWarper::new()           // Create warping context
GpuWarper::warp_channel()  // Warp single-channel f32 image
GpuWarper::warp_rgb()      // Warp RGB f32 image
```

**Performance Note**: CPU parallel warping is ~32% faster than GPU for 24+ megapixel images due to memory transfer overhead. GPU is recommended only when keeping data on GPU across multiple operations.

---

## GPU FFT Research (January 2026)

### Available Options

#### 1. gpu-fft (Pure Rust + CubeCL)
- **Crate**: `gpu-fft` on crates.io
- **Version**: 0.0.2 (March 2025)
- **Backend**: wgpu via CubeCL abstraction
- **License**: MIT

**Pros**:
- Pure Rust, no C dependencies
- Cross-platform (Vulkan, Metal, D3D12, WebGPU)
- Easy integration with wgpu ecosystem

**Cons**:
- Very new (created March 2025)
- Pre-release quality (0.x version)
- Missing optimizations (no radix-2 yet)
- Only 29 commits total
- ~750µs for FFT in benchmarks (unclear size)
- CubeCL dependency may have API instability

**Assessment**: NOT READY for production use. Too immature.

#### 2. vkfft-rs (Rust bindings for VkFFT)
- **Repository**: github.com/semio-ai/vkfft-rs
- **Backend**: Vulkan via vulkano
- **Upstream**: VkFFT (C library)

**Pros**:
- VkFFT is highly optimized, competes with cuFFT
- 1D, 2D, 3D FFT support
- Built-in convolution support
- VkFFT works on Nvidia, AMD, Intel, Apple, Raspberry Pi

**Cons**:
- Vulkano dependency (not wgpu)
- Safety concerns acknowledged by maintainers
- libSPIRV.a linking issues can cause segfaults
- No formal releases (pre-1.0)
- Only 30 stars, 13 commits

**Assessment**: Promising but risky. Would require vulkano instead of wgpu.

#### 3. Custom WGSL Implementation
Write FFT compute shaders directly in WGSL for wgpu.

**Pros**:
- Full control
- Integrates with existing imaginarium/wgpu infrastructure
- No additional dependencies

**Cons**:
- Significant development effort
- Complex to optimize (twiddle factors, bit reversal, shared memory)
- Maintenance burden

### RustFFT Performance Context

RustFFT (CPU) is highly optimized:
- **AVX-accelerated**: 5x-10x faster than RustFFT 4.0
- **Beats FFTW**: One of the industry-leading FFT libraries
- **NEON support**: Auto-detected on AArch64
- **Optimal sizes**: 2^n * 3^m for fastest paths

For typical astrophotography sizes (512x512 to 4096x4096):
- 512x512: ~few ms on CPU
- 1024x1024: ~10-20ms on CPU
- 2048x2048: ~50-100ms on CPU

GPU FFT typically shows benefits at larger sizes (>1024x1024) due to transfer overhead.

### Phase Correlation Specifics

Current implementation in `src/registration/phase_correlation/mod.rs`:
- Uses row-column decomposition (separable 2D FFT)
- FFT sizes are power-of-2 (128, 256, 512, 1024, ...)
- Main operations: 2x 1D FFT (forward), cross-power spectrum, 2x 1D FFT (inverse)

**Bottleneck Analysis**:
- FFT itself is well-optimized by RustFFT
- Transpose operations add overhead
- Cross-power spectrum is memory-bound
- GPU would need batched execution to amortize transfer costs

### Recommendation

**Status**: NOT VIABLE for immediate implementation

**Reasoning**:
1. No mature wgpu-based FFT library exists (gpu-fft is 0.0.2)
2. vkfft-rs would require switching from wgpu to vulkano
3. RustFFT is already highly competitive with GPU FFT for typical sizes
4. GPU transfer overhead may negate performance gains
5. Custom WGSL implementation is too complex for uncertain benefit

**Alternative Approaches**:
1. **Keep RustFFT** - Already very fast, mature, maintained
2. **Batch multiple correlations** - Process multiple frame pairs in parallel
3. **GPU post-FFT operations** - Move cross-power spectrum and peak finding to GPU
4. **Wait for gpu-fft maturity** - Re-evaluate in 6-12 months

### Sources

- [gpu-fft crate](https://crates.io/crates/gpu-fft)
- [gpu-fft GitHub](https://github.com/eugenehp/gpu-fft)
- [vkfft-rs GitHub](https://github.com/semio-ai/vkfft-rs)
- [VkFFT](https://github.com/DTolm/VkFFT)
- [RustFFT](https://github.com/ejmahler/RustFFT)
- [RustFFT 5.0 announcement](https://users.rust-lang.org/t/rustfft-5-0-0-experimental-1-now-faster-than-fftw/53049)

---

## GPU Acceleration Roadmap

### Implemented
- [x] GPU image warping (imaginarium/wgpu)
- [x] GPU sigma clipping (see `stacking/gpu/NOTES-AI.md`)
- [x] GPU star detection threshold mask (see `star_detection/gpu/NOTES-AI.md`)
- [x] Batch processing pipeline with async readback (see `stacking/gpu/NOTES-AI.md`)

### Research Complete
- [x] GPU FFT options evaluated - NOT VIABLE currently
- [x] GPU sigma clipping - parallel reduction strategies researched

### Skipped
- GPU FFT for phase correlation - no viable wgpu library exists

---

## GPU Sigma Clipping Research (January 2026)

### Problem Analysis

**Current CPU Implementation** (`src/stacking/sigma_clipped/mod.rs`):
- Per-pixel stack of N frames (typically 15-100 frames)
- Iterative algorithm: compute median/mean + std_dev, clip outliers, repeat
- Default: 3 iterations with sigma=2.5
- Uses SIMD-accelerated math (`math::median_f32_mut`, `math::sum_squared_diff`)

**GPU Challenge**: Unlike simple reductions (sum, min, max), sigma clipping requires:
1. Computing statistics (mean/median, std_dev) for each pixel stack
2. Filtering based on those statistics
3. Iterating until convergence (typically 2-3 iterations)

### Parallel Reduction Strategies

#### 1. NVIDIA Mark Harris Optimizations (Classic Reference)

From [NVIDIA's Optimizing Parallel Reduction](https://developer.download.nvidia.com/assets/cuda/files/reduction.pdf):

| Optimization | Speedup | Description |
|--------------|---------|-------------|
| Interleaved → Sequential addressing | 2× | Avoids shared memory bank conflicts |
| First add during global load | 2× | Halves number of blocks needed |
| Unroll last warp | 2× | Removes synchronization overhead |
| Complete unrolling | 1.4× | Eliminates loop overhead |
| Multiple elements per thread | 1.4× | Better instruction-level parallelism |
| **Total** | **30×** | Cumulative improvement |

**Key Insights**:
- Sequential addressing (contiguous memory) is faster than interleaved
- Workgroup shared memory enables fast intra-workgroup reductions
- Warp-level operations (shuffle instructions) bypass shared memory for last 32 elements
- GPU bandwidth: can achieve 62-73 GB/s for reductions on 32M elements

#### 2. wgpu/WebGPU Considerations

**Workgroup Shared Memory in WGSL**:
```wgsl
var<workgroup> shared_data: array<f32, 256>;

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(local_invocation_index) local_idx: u32) {
    // Load to shared memory
    shared_data[local_idx] = input[global_idx];
    workgroupBarrier();

    // Parallel reduction tree
    for (var stride = 128u; stride > 0u; stride >>= 1u) {
        if (local_idx < stride) {
            shared_data[local_idx] += shared_data[local_idx + stride];
        }
        workgroupBarrier();
    }
    // Result in shared_data[0]
}
```

**Limitations**:
- WebGPU only supports barriers within a workgroup (max 256 threads)
- Multi-block reductions require multiple kernel dispatches
- No subgroup/warp-level operations in standard WebGPU (extension exists but not universal)

#### 3. Sigma Clipping Specific Algorithm

**Proposed GPU Design**:

**Pass 1: Compute Statistics (per pixel stack)**
- Each thread handles one pixel position across all N frames
- Compute median (requires sorting) OR mean (simple reduction)
- Compute sum of squared differences for std_dev

**Pass 2: Mark Outliers**
- Simple threshold comparison: `abs(value - center) > sigma * stddev`
- Store mask or compact valid values

**Pass 3: Recompute Mean**
- Mean of non-rejected values only

**Iteration**: Repeat passes 1-3 for `max_iterations` times (typically 3)

#### 4. Per-Pixel Stack vs Per-Image Processing

**Current CPU approach** (per-pixel column-wise):
- Load all frames, process each pixel position's stack independently
- Frame count N typically 15-100
- Pixel count M typically 4-24 million

**GPU-friendly approach** (per-pixel parallel):
- Each workgroup handles a tile of pixels (e.g., 16×16 = 256 positions)
- Each thread computes statistics for one pixel stack
- Requires frame data to be accessible efficiently (texture cache or buffer)

**Memory Layout Challenge**:
- CPU uses row-major images, processes column-wise through stack
- GPU prefers coalesced memory access (adjacent threads read adjacent memory)
- May need transposed data layout or texture sampling

### GPU Median Computation

Computing exact median on GPU is challenging:
- Requires sorting each stack (N elements per pixel)
- Sorting networks work well for small N (<32)
- For larger N, use approximate methods:
  - Histogram-based median (discretize values, find bin)
  - Partial sorting (quickselect-style)
  - Use mean instead (less robust but much faster)

**Recommendation**: Use mean for GPU sigma clipping, as it:
- Is easily computable via parallel reduction
- Works well with iterative clipping (outliers removed progressively)
- Is what the final result uses anyway (mean of remaining values)

### Performance Expectations

**When GPU might help**:
- Large images (24+ megapixels) × many frames (50+)
- Data already on GPU (e.g., after GPU warping)
- Batch processing multiple stacks

**When CPU is likely better**:
- Small stacks (<20 frames): overhead dominates
- Data needs CPU→GPU transfer: ~10GB/s typical PCIe bandwidth
- Random access patterns (connected components, etc.)

**GPU Transfer Overhead Example**:
- 24MP × 50 frames × 4 bytes = 4.8 GB
- PCIe 3.0 x16: ~12 GB/s → 400ms transfer time
- CPU sigma clipping: likely <1s for this data
- GPU must provide significant speedup to overcome transfer

### Recommended Implementation Approach

1. **Implement GPU statistics kernel first** (mean + variance per pixel)
2. **Use mean-based clipping** (not median) for GPU path
3. **Keep data on GPU** if already there from warping
4. **Benchmark against CPU** with realistic datasets
5. **Fall back to CPU** for small stacks or when transfer-bound

### References

- [NVIDIA Parallel Reduction PDF](https://developer.download.nvidia.com/assets/cuda/files/reduction.pdf)
- [7 Step Optimization of Parallel Reduction](https://medium.com/@rimikadhara/7-step-optimization-of-parallel-reduction-with-cuda-33a3b2feafd8)
- [WebGPU Compute Shader Basics](https://webgpufundamentals.org/webgpu/lessons/webgpu-compute-shaders.html)
- [MSDGPU - GPU Mean and StdDev Library](https://github.com/KAdamek/GPU_mean_and_stdev)
- [Astropy Sigma Clipping](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clip.html)
- [Siril Stacking Documentation](https://siril.readthedocs.io/en/latest/preprocessing/stacking.html)

---

## Dependencies

- `imaginarium` - wgpu-based image processing
- `wgpu` - Cross-platform GPU API
- `rustfft` - CPU FFT (current, recommended)
