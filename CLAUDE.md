# Imaginarium

An image-processing library with **CPU (SIMD: SSE/AVX2/NEON) and GPU (wgpu compute) backends**, a unified CPU/GPU image buffer, and 12-format pixel polymorphism. A workspace member of Scenarium and a git submodule; consumed by `lens`/`lumos`. Pre-alpha, breaks freely.

`README.md` is the public-facing blurb; this file is the architecture map.

## Posture

- **No stability guarantees.** Rename, refactor, change signatures and rewrite callers. No compat shims.
- **GPU is optional, gated behind the `wgpu` feature** (`default = []`). Everything under `src/gpu/`, `GpuContext`, the `ops/*/gpu.rs` + `pipeline.rs` paths, and the `*Pipeline` / `Gpu*` re-exports in `lib.rs` only exist with `wgpu` on. CPU paths must compile and pass with the feature off.
- **SIMD correctness is cross-checked against scalar.** Every SIMD kernel has a scalar reference; tests compare the two. Don't add a SIMD path without the scalar fallback and the cross-check.
- **12 formats, always.** New ops and shaders handle the full `ALL_FORMATS` set (or explicitly declare a narrower supported list to `select_backend`). Half-covering the format matrix is a bug.

## Architecture

Six layers, bottom-up. A caller drives everything through a `ProcessingContext` and `ImageBuffer`s; ops pick a backend and the buffer transparently moves pixels between CPU and GPU.

1. **Image** — `Image` + `ImageDesc` hold raw, tightly-packed, 16-byte-aligned pixel bytes; `io.rs`/`tiff.rs` do PNG/JPG/TIFF.
2. **Color/format** — `ColorFormat` (`ChannelCount` × `ChannelSize` × `ChannelType`) is the format vocabulary; `conversion/` converts between any two formats.
3. **Processing context** — `ProcessingContext` owns the optional `GpuContext`; `ImageBuffer` abstracts CPU-vs-GPU residency via interior mutability.
4. **GPU wrapper** — `Gpu` (device+queue), `GpuImage` (a wgpu storage buffer + desc), `Slot` (async callback handoff).
5. **Ops** — `Blend`, `ContrastBrightness`, `Transform`; each exposes an `execute(&mut ProcessingContext, …)` that calls `select_backend` then routes to a CPU or GPU path.
6. **Kernels** — SIMD row/pixel functions (x86 runtime-detected, aarch64 NEON) and WGSL compute shaders.

### Image storage (`src/image/`)

`ImageDesc` (`src/image/mod.rs:24`) = `{ width, height, color_format }` — a single `new` constructor. Pixel data is **always tightly packed**: `row_bytes() == width * bytes_per_pixel`, `size_in_bytes() == height * row_bytes()`, no inter-row padding. There is no `stride` field. `Image` (`src/image/mod.rs:36`) holds `bytes: AVec<u8, ConstAlign<ALIGNMENT>>` (16-byte aligned for SIMD) plus the `desc`. Any row alignment a GPU backend needs lives entirely inside `GpuImage` (it derives an aligned stride, pads on upload, strips on download) — see the GPU wrapper section. `src/image/stride.rs` (the `align_stride`/`strip_stride_padding_from_slice` helpers) is therefore `#[cfg(feature = "wgpu")]`.

I/O (`src/image/io.rs`): PNG/JPG via the `image` crate, TIFF via `tiff.rs` (unlimited decoder for large astrophotography frames, U8/U16/U32/F32). `SUPPORTED_EXTENSIONS = ["png","jpg","jpeg","tiff","tif"]`. PNG save is U8/U16 only (no F32); JPG save needs `RGBA_U8` or `L_U8`.

### Color & conversion (`src/common/`)

`Color` (`src/common/color.rs`) is f32 RGBA in 0..1 with Rec. 709 `luminance()`; used by drawing and grayscale handling, not pixel storage. `ColorFormat` (`src/common/color_format.rs:31`) composes `ChannelCount` (L/LA/RGB/RGBA), `ChannelSize` (8/16/32-bit), `ChannelType` (UInt/Float). The 12 supported combos are macro-generated constants (`L_U8`…`RGBA_F32`); `ALL_FORMATS` lists all 12, `ALPHA_FORMATS` the 6 with alpha.

Conversion dispatch (`src/common/conversion/mod.rs:28`): `convert_image(from, to)` processes rows in parallel (`rayon`), trying `get_simd_row_converter(from_fmt, to_fmt)` first and falling back to `dispatch_convert_row_scalar`. SIMD lives in `conversion_simd/` (sse/avx/neon submodules); the converter type is `fn(&[u8], &mut [u8], usize)`. Covered fast paths: RGBA↔RGB, RGB→L, L→RGB, LA↔RGBA, U8↔U16, U16↔F32. `bench.rs` and `tests.rs` sit alongside.

`error.rs`: `Error` enum (`Io`, `InvalidExtension`, `UnsupportedColorType`, `UnsupportedFormat`, `InvalidColorFormat`, `SizeMismatch`, `Conversion`, `Encoding`, `Gpu`, `NoGpuContext`) + `Result<T>`. `image_diff.rs`: `max_pixel_diff`/`pixels_equal` for tests. `test_utils.rs`: cached lena fixtures + shared `test_gpu()`/`test_processing_context()` (GPU init is ~2s, so it's shared across tests).

### Processing context & buffers (`src/processing_context/`)

`ProcessingContext` (`mod.rs:16`) is the caller's entry point; holds `Option<GpuContext>`. `new()` attempts GPU init and falls back to CPU-only with a warning; `cpu_only()` forces CPU; `has_gpu()`/`gpu()`/`gpu_context()` expose it.

`ImageBuffer` (`image_buffer.rs:28`) is the CPU/GPU residency abstraction: `{ desc, storage: AtomicRefCell<Option<Storage>> }` where `Storage` is `Cpu(Image)` | `Gpu(GpuImage)`. The interior mutability lets `make_cpu()`/`make_gpu()` (and `make_*_mut`) upgrade residency in place — uploading via `GpuImage::from_image` or downloading via `GpuImage::to_image` only when the target side is missing. `to_cpu`/`to_gpu` consume and transfer. **This is the join point between the op layer and the GPU layer**: ops call `make_gpu`/`make_cpu` on their buffers as the chosen backend requires.

`GpuContext` (`gpu_context.rs:16`) = `{ gpu: Gpu, pipelines: HashMap<TypeId, Box<dyn GpuPipeline>> }`. `get_or_create::<T, F>(create)` lazily builds and caches one pipeline per concrete type — so each op's compute pipeline is compiled once and reused. `GpuPipeline` (`gpu_context.rs:9`) is a `Any + Debug + Send + Sync` marker enabling the `TypeId`-keyed downcast.

### GPU wrapper (`src/gpu/`, `wgpu` feature only)

`Gpu` (`mod.rs:10`) = `{ device: Arc<Device>, queue: Arc<Queue> }` (Arc so it clones across threads). `new()` requests a HighPerformance adapter with 1GB buffer limits; `wait()`/`wait_async()` poll to idle. `GpuImage` (`gpu_image.rs:46`) = a `STORAGE|COPY_SRC|COPY_DST` buffer + the (packed) `ImageDesc`. It owns **all** row alignment: `stride()` derives `align_stride(row_bytes)` (rows must start on a `u32` word so the WGSL `array<u32>` indexing works — a shader concern, not a buffer requirement; F32/RGBA_U8 are already aligned so this equals `row_bytes`). `from_image` uploads (padding rows out to the aligned stride, zero-copy when already aligned), `to_image`/`to_image_async` download via a staging buffer + `map_async` and **strip the padding back to a packed `Image`**, `clone_buffer` copies on-GPU. `ReadBuffer`/`WriteBuffer` (`gpu_image.rs:11,22`) are thin binding newtypes for shader bind groups. `Slot<T>` (`slot.rs:14`) is a lockless single-value handoff (`ArcSwapOption` + `Notify`) used by `to_image_async` to bridge the GPU map callback into async/await.

### Ops (`src/ops/`)

Each op is a small builder struct with an `execute(&self, ctx, …inputs, output: &mut ImageBuffer) -> Result<()>`. The shared shape:

- **`backend_selection.rs:16`** — `select_backend(ctx, buffers, cpu_formats, gpu_formats, op_name)`: asserts all buffers share a format, then prefers GPU when `wgpu` is on AND any buffer already lives on GPU AND the format is GPU-supported; otherwise CPU if supported; otherwise GPU-only if that's the only support; else error. An op forces GPU-only by passing an empty CPU format list (Transform does this).
- **`gpu_format.rs:5`** — `get_format_type(ColorFormat) -> u32` maps each format to a shader discriminant `FORMAT_L_U8`(0)…`FORMAT_RGBA_U16`(11); shaders `switch` on it.
- **Per-op submodule**: `mod.rs` (the public struct + `execute` + backend routing), `cpu.rs` (format dispatch → SIMD or scalar `apply_typed::<T>`), `gpu.rs` (`apply_gpu`: build uniform params, bind group, dispatch), `pipeline.rs` (the cached `Gpu*Pipeline`), and a `.wgsl` shader.

The three ops:

| Op | Type (`mod.rs`) | CPU | GPU | Shader |
|----|------|-----|-----|--------|
| Blend | `Blend { mode: BlendMode, alpha }`, `BlendMode { Normal, Add, Subtract, Multiply, Screen, Overlay }` (`blend/mod.rs:67`) | SSE4.1 (RGBA_U8/F32) / NEON + scalar `BlendApply` | `GpuBlendPipeline`, binds params+src+dst+output | `blend.wgsl` |
| Contrast/Brightness | `ContrastBrightness { contrast, brightness }`, formula `(x-mid)*contrast + mid + brightness` (`contrast_brightness/mod.rs:46`) | SSE4.1 / NEON + scalar (alpha preserved) | `GpuContrastBrightnessPipeline`, binds params+input+output | `contrast_brightness.wgsl` |
| Transform | `Transform { transform: Affine2, filter: FilterMode }`, `FilterMode { Nearest, Bilinear }` (default Bilinear); builders `scale`/`rotate`/`rotate_around`/`translate`/`affine`/`filter` (`transform/mod.rs:39`) | **none — GPU-only** | `GpuTransformPipeline`, applies `Affine2` + interpolation | `shader.wgsl` |

WGSL shaders treat storage buffers as `array<u32>` and pack/unpack per the `format_type` discriminant (12 cases). Workgroup size is 256; for narrow formats the blend shader packs multiple pixels per work item (L_U8: 4, LA_U8/L_U16: 2, else 1).

**End-to-end data flow** (Blend, GPU path): `Image` → `ImageBuffer::from_cpu` → `Blend::execute` → `select_backend` (sees a GPU buffer ⇒ GPU) → `make_gpu()` uploads any CPU buffers → `get_or_create::<GpuBlendPipeline>` → `apply_gpu` builds params+bind group, dispatches, `queue.submit()` → `to_image()` downloads when the caller wants CPU pixels back. CPU path is identical up to `select_backend`, then `make_cpu()` (no-op if already CPU) → `apply_cpu` → rayon-parallel SIMD/scalar kernel; result stays on CPU.

### Misc

`cpu_features.rs` — `X86Features { sse2, sse3, ssse3, sse4_1, avx2, fma }` cached in a `OnceLock`; `has_*` helpers gate the x86 SIMD dispatch at runtime (all-false off x86_64). `drawing.rs` — `draw_circle`/`draw_dot`/`draw_cross`/`draw_line` on f32 images (`L_F32`/`RGB_F32`); grayscale uses `Color::luminance()`, pixel access via `bytemuck::cast_slice_mut`.

## Project layout

- `src/lib.rs` — crate root: `cfg_x86_64!`/`cfg_aarch64!` macros, module decls, and the published surface (`pub use`s). GPU items are re-exported only under `#[cfg(feature = "wgpu")]`.
- `src/common/` — `color.rs`, `color_format.rs`, `conversion/` (mod dispatch + `conversion_scalar.rs` + `conversion_simd/` + `bench.rs` + `tests.rs`), `error.rs`, `image_diff.rs`, `test_utils.rs`.
- `src/image/` — `mod.rs` (`Image`/`ImageDesc`), `stride.rs`, `io.rs`, `tiff.rs`, `tests.rs`.
- `src/processing_context/` — `mod.rs` (`ProcessingContext`), `image_buffer.rs`, `gpu_context.rs`, `tests.rs`.
- `src/gpu/` — `mod.rs` (`Gpu`), `gpu_image.rs`, `slot.rs` (all `wgpu`-gated).
- `src/ops/` — `mod.rs`, `backend_selection.rs`, `gpu_format.rs`, and `blend/`, `contrast_brightness/`, `transform/` (each `mod.rs`/`cpu.rs`/`gpu.rs`/`pipeline.rs`/`.wgsl`; transform has no `cpu.rs`).
- `src/cpu_features.rs`, `src/drawing.rs`.
- `examples/` — `pipeline`, `pipeline_explicit`, `blend`, `brightness_contrast`, `conversion`, `transform` (+ `common/mod.rs` shared helpers).
- `test_resources/` — lena TIFF fixtures; `test_output/` — generated by tests.

## Dependencies of note

`wgpu` 29 (optional, metal/vulkan/dx12), `glam` 0.33 (`Affine2`/`Vec2`), `rayon` (parallel rows/pixels), `bytemuck` (Pod casts), `aligned-vec` (`AVec` 16-byte alignment), `atomic_refcell` (`ImageBuffer` interior mutability), `arc-swap` + `tokio` (optional, `Slot`/async GPU), `image` 0.25 + `tiff` 0.11 (I/O), `thiserror`, `strum`. Dev: `quickbench` (`../quickbench`, by relative path).

## Build & test

Edition 2024, toolchain pinned in `rust-toolchain.toml`. CPU-only is the default build; add `--features wgpu` for the GPU paths.

```sh
cargo check
cargo check --features wgpu
cargo test
cargo test --features wgpu
```

Benchmarks are `#[ignore]`d: `cargo test --release <bench> -- --ignored --nocapture`. Examples: `cargo run --release --example pipeline` (and `blend`/`transform`/…).
