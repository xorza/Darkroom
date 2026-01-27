# imaginarium - Implementation Notes (AI)

GPU/CPU image processing library with automatic backend selection.

## Key Modules

| Module | Description |
|--------|-------------|
| `image/mod.rs` | Core `Image` and `ImageDesc` types with file I/O (PNG, JPEG, TIFF) |
| `common/color_format.rs` | `ColorFormat`, `ChannelCount`, `ChannelSize`, `ChannelType` enums |
| `common/error.rs` | Error types (`Error`, `Result`) |
| `common/conversion/` | Color format conversion (scalar and SIMD implementations) |
| `gpu/mod.rs` | `Gpu` context wrapping wgpu device/queue |
| `gpu/gpu_image.rs` | `GpuImage` for GPU-resident image data |
| `processing_context/mod.rs` | `ProcessingContext` managing GPU resources and pipelines |
| `processing_context/image_buffer.rs` | `ImageBuffer` smart buffer with CPU/GPU transfer |
| `processing_context/gpu_context.rs` | `GpuContext` with cached pipeline management |
| `ops/blend/` | Image blending (Normal, Add, Subtract, Multiply, Screen, Overlay) |
| `ops/contrast_brightness/` | Brightness/contrast adjustment |
| `ops/transform/` | Affine transforms with filter modes |
| `ops/backend_selection.rs` | Automatic CPU/GPU backend selection |

## Key Types

```rust
Image              // CPU image with AVec<u8> (16-byte aligned), desc, file I/O, format conversion
ImageDesc          // { width: usize, height: usize, stride: usize, color_format }
ImageBuffer        // Smart buffer with automatic CPU/GPU transfer (AtomicRefCell)
Storage            // Cpu(Image) | Gpu(GpuImage)
ColorFormat        // { channel_count, channel_size, channel_type }
ChannelCount       // L | LA | Rgb | Rgba
ChannelSize        // _8bit | _16bit | _32bit
ChannelType        // UInt | Float
Gpu                // wgpu device/queue wrapper
GpuImage           // GPU-resident image with upload/download
ProcessingContext  // GPU context manager with pipeline caching
GpuContext         // Holds Gpu + cached pipelines (GpuPipeline trait)
```

## Operations

```rust
Blend              // { mode: BlendMode, alpha: f32 }
BlendMode          // Normal | Add | Subtract | Multiply | Screen | Overlay
ContrastBrightness // { contrast: f32, brightness: f32 }
Transform          // { transform: Affine2, filter: FilterMode }
FilterMode         // Nearest | Linear
```

## Color Formats

12 supported formats: L/LA/RGB/RGBA × U8/U16/F32

Constants: `ColorFormat::RGBA_U8`, `ColorFormat::RGB_F32`, etc.

Arrays: `ALL_FORMATS`, `ALPHA_FORMATS`

## SIMD Optimizations

Platform-specific optimizations in `common/conversion/`:
- SSE2/SSSE3/AVX2 (x86_64)
- NEON (aarch64)

Optimized paths:
- RGBA↔RGB (SSSE3/AVX2)
- RGB↔L (SSE2/AVX2)
- LA↔RGBA (SSSE3/AVX2)
- U8↔U16 (SSE2/AVX2)
- U16↔F32 (SSE2/AVX2)

## Architecture Patterns

- Operations have `apply_cpu()`, `apply_gpu()`, and `execute()` methods
- `execute()` auto-selects backend based on data location and format support
- `ImageBuffer` uses interior mutability (`AtomicRefCell`) for CPU/GPU conversion
- GPU pipelines cached in `GpuContext` via `GpuPipeline` trait
- wgpu compute shaders for GPU operations

## Dependencies

common, anyhow, wgpu, pollster, image, tiff, bytemuck, strum, rayon, atomic_refcell
