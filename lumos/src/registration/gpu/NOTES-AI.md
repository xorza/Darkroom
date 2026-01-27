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

### Research Complete
- [x] GPU FFT options evaluated - NOT VIABLE currently

### Not Started
- [ ] GPU sigma clipping (parallel reduction)
- [ ] GPU star detection (threshold + connected components)
- [ ] Batch processing pipeline (overlapped compute/transfer)

---

## Dependencies

- `imaginarium` - wgpu-based image processing
- `wgpu` - Cross-platform GPU API
- `rustfft` - CPU FFT (current, recommended)
