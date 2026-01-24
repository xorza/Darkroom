# Scenarium Implementation Notes (AI)

This file captures current implementation details and internal structure for AI agents.

## Project Overview

Scenarium is a Rust-based framework for building node-based data processing pipelines with async-capable function
execution. The repository is a Cargo workspace containing a core graph library, shared utilities, and a visual
egui-based editor.

**License:** AGPL  
**Build System:** Cargo workspace with 5 member crates

## Repository Layout

```
scenarium/
├── common/          # Shared utilities and helper macros
├── scenarium/       # Core graph library with execution engine
├── imaginarium/     # GPU/CPU image processing library
├── lumos/           # Astrophotography image processing library
├── prism/           # Visual egui-based graph editor
├── palantir/        # Image processing funclib (imaginarium adapter)
├── test_resources/  # Sample graphs and media for tests
├── test_output/     # Output folder created by unit tests
└── deprecated_code/ # Older .NET/QML editors (historical)
```

## Crates

### common

Shared utilities used across the workspace.

**Key modules:**
- `key_index_vec.rs` - Generic key-indexed vector with HashMap lookup; supports compaction with guards
- `shared.rs` - Thread-safe `Shared<T>` wrapper around `Arc<Mutex<T>>`
- `lambda.rs` - Async lambda patterns for callback support
- `macros.rs` - ID type generation macros (`id_type!`), async lambda macros
- `serde.rs` - Generic serialization (YAML, JSON, binary via bincode)
- `serde_lua.rs` - Lua-specific serialization support
- `file_format.rs` - Format detection and selection
- `pause_gate.rs` - Synchronization primitive for pausing/resuming async operations
- `ready_state.rs` - Readiness synchronization using tokio barriers
- `slot.rs` - Wait-friendly slot for async value passing
- `bool_ext.rs` - `BoolExt` trait with `then_else` helpers

**Dependencies:** tokio, serde, tracing, bumpalo, lz4_flex, bitcode

### graph

Core data structures for node-based graph execution with async lambdas.

**Key modules:**

- `graph.rs` - Core `Graph` struct with `NodeId`, `Node`, `Input`, `Binding`, `Event`
- `function.rs` - Function library definitions (`FuncLib`, `Func`, `FuncInput`, `FuncOutput`)
- `data.rs` - Data type and value systems (`DataType`, `StaticValue`, `DynamicValue`)
- `execution_graph.rs` - Execution scheduling and state management
- `func_lambda.rs` - Async function lambdas (`FuncLambda`, `AsyncLambdaFn`)
- `event_lambda.rs` - Async event callbacks (`EventLambda`)
- `context.rs` - Context registry for invocation environments (`ContextManager`)
- `worker.rs` - Background tokio thread for graph execution
- `compute.rs` - Node invocation and value conversion

**Elements (Built-in Functions):**
- `elements/basic_funclib.rs` - Math operations (add, subtract, multiply, divide, power, log, etc.)
- `elements/worker_events_funclib.rs` - Timer/frame events with FPS tracking
- `elements/lua/` - Lua function loading and invocation via mlua

**Dependencies:** common, tokio, serde, uuid, anyhow, thiserror, strum, mlua (Lua54), glam, hashbrown, criterion

### scenarium-editor

Visual egui-based editor for graph creation and execution.

**Core Application:**
- `main.rs` - eframe entry point with window setup
- `app_data.rs` - Application state management (`AppData`)
- `main_ui.rs` - Top-level UI rendering

**Model Layer:**
- `model/view_graph.rs` - Editor view representation with pan/zoom/selection
- `model/view_node.rs` - Per-node view state (position)
- `model/graph_ui_action.rs` - User actions enum for undo/redo
- `model/config.rs` - Persistent configuration

**GUI Components:**
- `gui/graph_ui.rs` - Main graph canvas and interaction
- `gui/node_ui.rs` - Individual node rendering
- `gui/graph_layout.rs` - Layout computation and caching
- `gui/connection_ui.rs` - Connection rendering with bezier curves
- `gui/connection_breaker.rs` - Tool for breaking/rerouting connections
- `gui/const_bind_ui.rs` - Const input binding badges and editors
- `gui/node_details_ui.rs` - Side panel for selected node
- `gui/new_node_ui.rs` - Dialog for adding new nodes
- `gui/graph_background.rs` - Dotted grid background
- `gui/polyline_mesh.rs` - Polyline mesh generation
- `gui/log_ui.rs` - Status/log panel
- `gui/style.rs` - Theme and styling centralization

**UI Primitives:**
- `common/button.rs` - Custom button with styling
- `common/toggle_button.rs` - Toggle button with state
- `common/drag_value.rs` - Inline draggable value editor
- `common/bezier_helper.rs` - Bezier intersection and sampling
- `common/undo_stack.rs` - Undo/redo implementations

**Dependencies:** common, graph, egui, eframe, wgpu, tokio, serde, rayon, rfd, arc-swap, bumpalo, lz4_flex

### imaginarium

GPU/CPU image processing library with automatic backend selection.

**Key modules:**

- `image/mod.rs` - Core `Image` and `ImageDesc` types with file I/O (PNG, JPEG, TIFF)
- `common/color_format.rs` - `ColorFormat`, `ChannelCount`, `ChannelSize`, `ChannelType` enums
- `common/error.rs` - Error types (`Error`, `Result`)
- `common/conversion/` - Color format conversion (scalar and SIMD implementations)
- `gpu/mod.rs` - `Gpu` context wrapping wgpu device/queue
- `gpu/gpu_image.rs` - `GpuImage` for GPU-resident image data
- `processing_context/mod.rs` - `ProcessingContext` managing GPU resources and pipelines
- `processing_context/image_buffer.rs` - `ImageBuffer` smart buffer with CPU/GPU transfer
- `processing_context/gpu_context.rs` - `GpuContext` with cached pipeline management
- `ops/blend/` - Image blending (Normal, Add, Subtract, Multiply, Screen, Overlay)
- `ops/contrast_brightness/` - Brightness/contrast adjustment
- `ops/transform/` - Affine transforms with filter modes
- `ops/backend_selection.rs` - Automatic CPU/GPU backend selection

**Key types:**

```rust
Image              // CPU image with bytes, desc, file I/O, format conversion
ImageDesc          // { width: usize, height: usize, stride: usize, color_format }
ImageBuffer        // Smart buffer with automatic CPU/GPU transfer (AtomicRefCell)
Storage            // Cpu(Image) | Gpu(GpuImage)
ColorFormat        // { channel_count, channel_size, channel_type }
ChannelCount       // Gray | GrayAlpha | Rgb | Rgba
ChannelSize        // _8bit | _16bit | _32bit
ChannelType        // UInt | Float
Gpu                // wgpu device/queue wrapper
GpuImage           // GPU-resident image with upload/download
ProcessingContext  // GPU context manager with pipeline caching
GpuContext         // Holds Gpu + cached pipelines (GpuPipeline trait)
```

**Operations:**

```rust
Blend              // { mode: BlendMode, alpha: f32 }
BlendMode          // Normal | Add | Subtract | Multiply | Screen | Overlay
ContrastBrightness // { contrast: f32, brightness: f32 }
Transform          // { transform: Affine2, filter: FilterMode }
FilterMode         // Nearest | Linear
```

**Color formats (12 supported):**
- Gray/GrayAlpha/RGB/RGBA × U8/U16/F32
- Constants: `ColorFormat::RGBA_U8`, `ColorFormat::RGB_F32`, etc.
- `ALL_FORMATS` and `ALPHA_FORMATS` arrays

**Architecture patterns:**
- Operations have `apply_cpu()`, `apply_gpu()`, and `execute()` methods
- `execute()` auto-selects backend based on data location and format support
- `ImageBuffer` uses interior mutability (`AtomicRefCell`) for CPU/GPU conversion
- GPU pipelines cached in `GpuContext` via `GpuPipeline` trait
- wgpu compute shaders for GPU operations

**Dependencies:** common, anyhow, wgpu, pollster, image, tiff, bytemuck, strum, rayon, atomic_refcell

### vision

Image processing function library adapting imaginarium operations to the node-based workflow.

**Key modules:**
- `image_funclib.rs` - Image processing functions and type definitions
- `vision_ctx.rs` - `VisionCtx` with `ProcessingContext` for GPU/CPU image operations

**Key types:**
- `ImageFuncLib` - Function library with image processing nodes
- `Image` - Wrapper around `imaginarium::ImageBuffer` implementing `CustomValue`
- `VisionCtx` - Context holding `imaginarium::ProcessingContext` for GPU/CPU dispatch
- `ConversionFormat` - Enum for all 12 color format conversion targets
- `IMAGE_DATA_TYPE` - Lazy-initialized custom `DataType` for images
- `BLENDMODE_DATATYPE` - Lazy-initialized enum `DataType` for blend modes
- `CONVERSION_FORMAT_DATATYPE` - Lazy-initialized enum `DataType` for conversion formats
- `VISION_CTX_TYPE` - Lazy-initialized `ContextType` for context manager

**Current functions:**
- `brightness_contrast` - Adjusts image brightness and contrast
- `transform` - Applies affine transformations (translate, scale, rotate)
- `save_image` - Saves image to file
- `convert` - Converts image to different color format (enum input)
- `blend` - Blends two images with configurable mode and alpha

**Dependencies:** common, graph, imaginarium, anyhow, strum, strum_macros

### lumos

Astrophotography image processing library for loading, calibrating, and stacking astronomical images.

**Key modules:**

- `astro_image/mod.rs` - `AstroImage` for loading FITS and RAW camera files
- `stacking/mod.rs` - Image stacking algorithms (mean, median, sigma-clipped mean)
- `calibration_masters.rs` - Master dark/flat/bias frame management
- `math.rs` - SIMD-accelerated math utilities (ARM NEON, x86 SSE4)

**Key types:**

```rust
AstroImage         // Astronomical image with metadata and f32 pixels
AstroImageMetadata // FITS metadata (object, instrument, exposure, bitpix, etc.)
ImageDimensions    // { width: usize, height: usize, channels: usize }
BitPix             // FITS pixel type enum (UInt8, Int16, Int32, Int64, Float32, Float64)
StackingMethod     // Mean | Median | SigmaClippedMean(SigmaClipConfig)
SigmaClipConfig    // { sigma, max_iterations }
CacheConfig        // { cache_dir, keep_cache, available_memory, progress } - cache directory, cleanup, memory override, progress callback
CacheError         // Error type for cache operations (NoPaths, ImageLoad, DimensionMismatch, I/O errors)
CacheProgress      // { current, total, stage } - progress info for callbacks
CacheStage         // Loading | Processing
FrameType          // Dark | Flat | Bias | Light
CalibrationMasters // Container for master dark/flat/bias frames
```

**Module structure:**
- `astro_image/mod.rs` - Main module with `AstroImage`, `BitPix`, `ImageDimensions`, `from_file()`, `calibrate()`
- `astro_image/fits.rs` - FITS file loading via fitsio
- `astro_image/rawloader.rs` - RAW loading via rawloader (pure Rust)
- `astro_image/libraw.rs` - RAW loading via libraw-rs (C library fallback)
- `stacking/mod.rs` - `StackingMethod`, `FrameType`, `stack_frames()` dispatch
- `stacking/cache.rs` - `ImageCache` with memory-mapped binary cache and `process_chunked()` for shared chunked processing, `CacheError` for error handling
- `stacking/cache_config.rs` - `CacheConfig` struct with progress callbacks and `compute_optimal_chunk_rows()` for adaptive chunk sizing based on available memory and image dimensions (uses sysinfo)
- `stacking/mean/` - Mean stacking module
  - `mod.rs` - Public API with `stack_mean_from_images()`
  - `cpu.rs` - SIMD dispatch proxy (NEON/SSE/scalar)
  - `scalar.rs` - Scalar fallback implementation
  - `neon.rs` - ARM NEON SIMD implementation
  - `sse.rs` - x86 SSE2 SIMD implementation
- `stacking/median/` - Median stacking module (memory-efficient via mmap)
  - `mod.rs` - Public API with `stack_median_from_paths()`, `MedianStackConfig`
  - `cpu.rs` - SIMD dispatch proxy (NEON/SSE/scalar)
  - `scalar.rs` - Scalar median using quickselect
  - `neon.rs` - ARM NEON SIMD sorting networks for small arrays (≤16 elements)
  - `sse.rs` - x86 SSE2 SIMD sorting networks for small arrays (≤16 elements)
- `stacking/sigma_clipped/` - Sigma-clipped mean stacking (memory-efficient via mmap)
  - `mod.rs` - Public API with `stack_sigma_clipped_from_paths()`, `SigmaClippedConfig`, `SigmaClipConfig`
  - `cpu.rs` - Proxy to scalar implementation
  - `scalar.rs` - Iterative sigma clipping algorithm
- `calibration_masters.rs` - `CalibrationMasters` struct with `from_directory()`, `load_from_directory()`, `save_to_directory()`
- `math.rs` - SIMD math: `sum_f32()`, `mean_f32()`, `sum_squared_diff()`

**Conversions:**
- `From<AstroImage> for imaginarium::Image` - converts f32 pixels to Image
- `From<imaginarium::Image> for AstroImage` - converts to GRAY_F32 or RGB_F32, removes stride padding

**Calibration:**
- `AstroImage::calibrate(master_dark, master_flat)` - applies dark subtraction and flat field correction
- `CalibrationMasters::from_directory()` - stacks raw calibration frames into masters
- `CalibrationMasters::load_from_directory()` - loads existing master TIFF files
- Master filenames include stacking method: `master_dark_median.tiff`

**RAW file loading:**
- Tries `rawloader` first (pure Rust, faster, limited camera support)
- Falls back to `libraw` (C library via libraw-rs, broader camera support)
- Supports RAF, CR2, CR3, NEF, ARW, DNG formats
- Data normalized to 0.0-1.0 range

**FITS file loading:**
- Via `fitsio` crate with cfitsio backend
- Reads primary HDU image data as f32
- Extracts metadata (OBJECT, INSTRUME, TELESCOP, DATE-OBS, EXPTIME, BITPIX)

**SIMD math utilities:**
- Platform-specific optimizations for ARM NEON (aarch64) and x86 SSE4
- `sum_f32()` - SIMD-accelerated sum of f32 values
- `mean_f32()` - SIMD-accelerated mean calculation
- `sum_squared_diff()` - SIMD-accelerated sum of squared differences from mean
- Scalar fallback for unsupported platforms and small arrays (<4 elements)

**Demosaic module (`astro_image/demosaic/`):**
- Bilinear demosaicing of Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
- Multi-threaded row-based processing via rayon for large images (128x128+)
- SIMD acceleration: SSE3 (x86_64) and NEON (aarch64) processing 4 pixels in parallel
- Pre-computed CFA pattern lookup tables for branchless color determination
- Optimized scalar fallback with fast-path for interior pixels
- Automatic backend selection based on image size and platform capabilities
- Module structure:
  - `mod.rs` - Main module with `CfaPattern`, `BayerImage`, `demosaic_bilinear()`, and parallel row processors
  - `bayer/mod.rs` - Submodule organizing Bayer demosaic implementations
  - `bayer/scalar.rs` - Scalar (non-SIMD) bilinear demosaicing implementation
  - `bayer/simd_sse3.rs` - x86_64 SSE3 SIMD implementation
  - `bayer/simd_neon.rs` - ARM aarch64 NEON SIMD implementation
  - `bayer/bench.rs` - Criterion benchmarks for Bayer demosaic (feature-gated)
  - `bayer/tests.rs` - Comprehensive unit tests
  - `xtrans/` - Submodule for X-Trans demosaicing (see below)
- **Test coverage:** Channel preservation (R/G/B at known positions), NaN/Infinity detection, extreme values (zeros, max), corner pixels, asymmetric margins, non-square images, gradient patterns, SIMD vs scalar consistency
- Benchmarks in `lumos/benches/`: `demosaic.rs` (Bayer), `xtrans.rs` (X-Trans)
- Run with: `cargo bench --package lumos --features bench demosaic` or `xtrans`

**Sensor module (`astro_image/sensor.rs`):**
- `SensorType` enum: `Monochrome`, `Bayer(CfaPattern)`, `XTrans`, `Unknown`
- `detect_sensor_type(filters, colors)` - detects sensor type from libraw metadata
- `cfa_pattern_from_filters(filters)` - converts libraw filters field to `CfaPattern`
- Monochrome detection: `filters == 0` or `colors == 1`
- X-Trans detection: `filters == 9` (Fujifilm sensors)
- Other non-Bayer patterns return `Unknown`

**X-Trans module (`astro_image/demosaic/xtrans/`):**
- Bilinear demosaicing for Fujifilm X-Trans sensors (6x6 CFA pattern)
- `XTransPattern` - 6x6 pattern array with values 0=Red, 1=Green, 2=Blue
- `XTransImage` - Raw X-Trans data with margins, dimensions, and pattern
- `demosaic_xtrans_bilinear()` - Bilinear interpolation demosaicing with SIMD acceleration
- Multi-threaded row-based processing via rayon for large images
- SIMD acceleration: SSE4.1 (x86_64) and NEON (aarch64) for neighbor lookup
- **Key optimizations:**
  - `OffsetList` - Pre-computed (dy, dx) offsets for each color at each of 36 pattern positions, eliminating pattern lookups and modulo operations in hot loops
  - `LinearOffsetList` - Pre-computed linear offsets (`dy * stride + dx`) eliminating multiply operations in inner loop
  - `NeighborLookup` - 6x6 lookup table of OffsetLists for each color
  - `LinearNeighborLookup` - Same with linear offsets, requires stride at construction time
  - 4 independent accumulators in SIMD for instruction-level parallelism (ILP)
- Module structure:
  - `mod.rs` - `XTransPattern`, `XTransImage` types with validation
  - `scalar.rs` - Scalar bilinear demosaic with lookup tables and detailed optimization docs
  - `simd_sse4.rs` - x86_64 SSE4.1 SIMD using linear offsets and 4-accumulator ILP
  - `simd_neon.rs` - ARM aarch64 NEON SIMD using same optimization strategy
  - `bench.rs` - Criterion benchmarks for X-Trans demosaic (feature-gated)
  - `integration_tests.rs` - Integration tests requiring RAF files
- **Test coverage:** Channel preservation, NaN/Infinity detection, extreme values, corner pixels, asymmetric margins, non-square images, gradient patterns, SIMD vs scalar consistency

**Dependencies:** common, imaginarium, fitsio, rawloader, libraw-rs, anyhow, rayon, strum_macros

## Key Data Structures

### Graph Elements

```rust
NodeId(uuid)      // Unique node identifier
FuncId(uuid)      // Unique function identifier  
TypeId(uuid)      // Custom type identifier
PortAddress       // { target_id: NodeId, port_idx: usize }
EventRef          // { node_id: NodeId, event_idx: usize }
```

### Binding System

```rust
Binding:
  - None                    // Unconnected
  - Const(StaticValue)      // Constant value
  - Bind(PortAddress)       // Data flow from another node's output
```

### Execution State

```rust
InputState: Changed | Unchanged
OutputUsage: Skip | Needed
ExecutionBehavior: Impure | Pure | Once
NodeBehavior: AsFunction | Once
```

### Value Types

```rust
DataType: Null | Float | Int | Bool | String | Array | Custom(TypeId)
StaticValue: Serializable constants (f64 equality via to_bits())
DynamicValue: Runtime values including Arc<dyn Any + Send + Sync> for Custom variant (shallow clone)
```

## Architecture Patterns

### Lambda-Based Function Execution
- Functions defined as async Rust closures (`FuncLambda`)
- Lua scripting integration via mlua
- Async/await support throughout execution
- `async_lambda!` macro reduces boilerplate

### Execution Graph Scheduling
- DFS-based topological ordering with cycle detection
- Three-phase scheduling: backward (identify needs), forward (mark execution), terminal discovery
- Input state tracking for pure function optimization
- Output usage counting to skip unnecessary computations
- Reusable `processing_stack` cache for cycle detection

### Event System
- Nodes emit named events with subscriber lists
- Event lambdas run in async worker loop
- Frame events with configurable frequency (`FpsEventState`)

### Worker Pattern
- Background tokio task receives messages (Update, Event, Clear, ExecuteTerminals)
- Execution callbacks with stats (elapsed time, node timings, missing inputs)
- Event loop with stop flag and notify broadcast
- `EventLoopHandle` for controlling event loops

### UI Architecture
- Immediate-mode with egui
- Arena allocation (bumpalo) for frame-local data
- Persistent layout caching with in-place updates (`CompactInsert`)
- Undo/redo via action replay with LZ4 compression
- Cycle detection prevents invalid graph connections

### State Management
- `AnyState` - Type-erased HashMap for per-node state
- `SharedAnyState` - Arc<Mutex<AnyState>> for event state sharing
- `ContextManager` - Lazy-initialized context registry
- `Slot` - Wait-friendly value passing between async boundaries

## ID Generation

The `id_type!` macro generates strongly-typed UUID wrappers:
- Implements Debug, Clone, Copy, Eq, Hash, serde traits
- `From<&str>`/`From<String>` for parsing UUIDs
- `from_u128` for const-friendly initialization

## Serialization

- `common::serialize` returns `Vec<u8>`; `deserialize` accepts bytes
- Text formats (YAML/JSON) are UTF-8 bytes
- Binary format uses bincode
- `FileFormat` enum for format selection and auto-detection by extension
- `StaticValue` uses `f64::to_bits()` for deterministic float equality

## Testing

- Unit tests in-module via `#[cfg(test)]` blocks
- `test_graph()` and `test_func_lib()` fixtures for reproducible testing
- Criterion benchmarks in `scenarium/benches/b1.rs`
- `TestFuncHooks` with Arc callbacks for async test support

## Editor Features

- Node drag, selection, reordering (selected nodes render on top)
- Connection creation with bezier curves and gradient coloring
- Connection breaker tool for rerouting
- Const input badges with inline editing
- Cache toggle per node (Once behavior)
- Impure function status indicator
- Undo/redo with action-based snapshots (LZ4 compressed)
- Fit-all and view-selected camera controls
- Autorun toggle for automatic execution
- Cycle detection before applying connections
- Dotted grid background with zoom-aware spacing

## Style System

`Style` centralizes all UI appearance constants:
- Initialized from `StyleSettings` (TOML serializable)
- Scaled by zoom factor via `Style::new(scale)`
- Contains `NodeStyle`, `ConnectionStyle`, `GraphBackgroundStyle`
- Applied to egui visuals via `Style::apply_to_egui`

### Port Colors

`PortColors` struct provides unified port color selection:
- `NodeStyle::port_colors(PortKind) -> PortColors` returns base/hover color pair
- `PortColors::select(hovered: bool) -> Color32` picks the appropriate color
- Used by: `render_ports()`, `build_const_bind_style()`, `ConnectionBezierStyle::build()`

### Value Editor

`StaticValueEditor` in `common/value_editor.rs` provides unified editing for `StaticValue` types:
- Handles Int, Float, Enum, FsPath value types
- Builder pattern with `.pos()`, `.align()`, `.style()` configuration
- Used by `const_bind_ui.rs` for inline const binding editing
- Extensible for new value types via match arm addition

### ID Salt Helpers

`common/id_salt.rs` provides standardized ID generation for egui persistent IDs:
- `NodeIds` - node body, drag start, status indicators
- `PortIds` - port interaction areas
- `ConstBindIds` - const binding connections and value editors
- Returns tuples that hash consistently for `ui.make_persistent_id()`

### Drag State

`DragState<T>` in `common/drag_state.rs` manages drag operations:
- Stores start value on drag_started, returns it on drag_stopped
- `DragResult` enum: `Idle`, `Started`, `Dragging`, `Stopped { start_value }`
- Used by `node_ui.rs` for node position dragging with undo support
- Type-safe with Clone + Default + Send + Sync bounds

### Graph Interaction State

`GraphInteractionState` in `gui/interaction_state.rs` centralizes UI interaction mode tracking:
- `InteractionMode` enum: `Idle`, `BreakingConnections`, `DraggingNewConnection`, `PanningGraph`
- Encapsulates `ConnectionBreaker` state within the interaction state
- Clean API: `start_breaking()`, `start_dragging_connection()`, `start_panning()`, `reset_to_idle()`
- `breaker()` returns `Option<&ConnectionBreaker>` only when in breaking mode
- Used by `graph_ui.rs` to separate interaction logic from rendering

## Undo System

Two implementations available:
- `FullSerdeUndoStack` - Stores full serialized snapshots
- `ActionUndoStack` - Stores `GraphUiAction` values with binary compression

Both support byte limits for memory management.

## Configuration

- `config.toml` - Current graph path, window geometry
- Window position/size persists via eframe
- `StyleSettings` supports TOML persistence

## Dependencies Overview

**Core Runtime:** tokio, serde, anyhow/thiserror, uuid  
**Data:** hashbrown, glam, bumpalo, lz4_flex  
**Scripting:** mlua (Lua 5.4)  
**UI:** egui, eframe, wgpu, rayon, rfd  
**Utilities:** strum, arc-swap, tracing
