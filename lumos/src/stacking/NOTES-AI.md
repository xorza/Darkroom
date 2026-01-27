# Stacking Module - Implementation Notes (AI)

## Overview

Image stacking algorithms for astrophotography, including mean, median, sigma-clipped, weighted mean, drizzle super-resolution, and pixel rejection methods.

## Module Structure

| Module | Description |
|--------|-------------|
| `mod.rs` | `StackingMethod`, `FrameType`, `ImageStack`, dispatch |
| `error.rs` | `Error` enum for stacking operations |
| `cache.rs` | `ImageCache` with memory-mapped binary cache |
| `cache_config.rs` | `CacheConfig` with adaptive chunk sizing |
| `mean/` | Mean stacking (SIMD: NEON/SSE/scalar) |
| `median/` | Median stacking via mmap (SIMD sorting networks) |
| `sigma_clipped/` | Sigma-clipped mean via mmap |
| `weighted/` | Weighted mean with quality-based frame weights |
| `rejection.rs` | Pixel rejection algorithms |
| `drizzle.rs` | Drizzle super-resolution stacking |
| `local_normalization.rs` | Local normalization (tile-based, PixInsight-style) |

## Key Types

```rust
StackingMethod     // Mean | Median | SigmaClippedMean | WeightedMean
FrameType          // Dark | Flat | Bias | Light
ImageStack         // Main stacking orchestrator
CacheConfig        // { cache_dir, keep_cache, available_memory }
WeightedConfig     // { weights, rejection, cache }
FrameQuality       // { snr, fwhm, eccentricity, noise, star_count }
RejectionMethod    // None | SigmaClip | WinsorizedSigmaClip | LinearFitClip | PercentileClip | Gesd
DrizzleConfig      // { scale, pixfrac, kernel, min_coverage, fill_value }
DrizzleKernel      // Square | Point | Gaussian | Lanczos
NormalizationMethod // None | Global | Local(LocalNormalizationConfig)
LocalNormalizationConfig // { tile_size, clip_sigma, clip_iterations }
TileNormalizationStats   // Per-tile median and scale statistics
LocalNormalizationMap    // Correction map for applying normalization
```

## Rejection Methods

| Method | Best For | Description |
|--------|----------|-------------|
| SigmaClip | General use | Iterative kappa-sigma clipping |
| WinsorizedSigmaClip | Preserving data | Replace outliers with boundary values |
| LinearFitClip | Sky gradients | Fits line to pixel stack, rejects deviants |
| PercentileClip | Small stacks | Simple low/high percentile rejection |
| GESD | Large stacks (>50) | Generalized Extreme Studentized Deviate Test |

---

## Local Normalization (IMPLEMENTED)

### Research Summary

**Sources:**
- [PixInsight Local Normalization](https://chaoticnebula.com/pixinsight-local-normalization/)
- [Astro Pixel Processor LNC FAQ](https://www.astropixelprocessor.com/community/faq/what-exactly-does-the-lnc-and-when-do-i-change-the-default-values-to-something-else/)
- [Siril Stacking Documentation](https://siril.readthedocs.io/en/stable/preprocessing/stacking.html)

### Algorithm Overview

Local Normalization corrects illumination differences across frames by matching brightness **locally** rather than globally. This handles:
- Vignetting (darker corners, brighter center)
- Sky gradients (light pollution, moon, twilight)
- Session-to-session brightness variations

**Key benefit**: Dramatically improves pixel rejection in final integration by ensuring all subframes have matched, flat backgrounds.

### Implementation Details

**Module**: `local_normalization.rs`

**Algorithm Steps**:
1. **Tile Division**: Divide image into tiles (default: 128×128, configurable 64-256)
2. **Per-Tile Statistics**: Compute sigma-clipped median and MAD for each tile (reuses `sigma_clipped_median_mad()` from `star_detection/constants`)
3. **Compute Correction Factors**:
   - `offset = ref_median` (per tile)
   - `scale = ref_scale / target_scale` (clamped to avoid division by near-zero)
   - `target_median` stored for the correction formula
4. **Smooth Interpolation**: Bilinear interpolation between tile centers (segment-based for efficiency)
5. **Apply Correction**: `pixel_corrected = (pixel - target_median) * scale + ref_median`

**Key Types**:
```rust
// Normalization method enum
pub enum NormalizationMethod {
    None,              // No normalization
    Global,            // Match overall median and scale (default)
    Local(LocalNormalizationConfig), // Tile-based matching
}

// Configuration
pub struct LocalNormalizationConfig {
    pub tile_size: usize,        // Default: 128, Range: 64-256
    pub clip_sigma: f32,         // Default: 3.0
    pub clip_iterations: usize,  // Default: 3
}

// Presets
LocalNormalizationConfig::fine()   // 64px tiles - steep gradients
LocalNormalizationConfig::coarse() // 256px tiles - stability

// Per-tile statistics
pub struct TileNormalizationStats {
    medians: Vec<f32>,
    scales: Vec<f32>,
    tiles_x, tiles_y, tile_size, width, height
}

// Correction map
pub struct LocalNormalizationMap {
    offsets, scales, target_medians: Vec<f32>,
    centers_x, centers_y: Vec<f32>,
    tiles_x, tiles_y, width, height
}
```

**Convenience Functions**:
```rust
// Compute normalization map from reference and target
compute_normalization_map(reference, target, width, height, config) -> LocalNormalizationMap

// Apply normalization in one step
normalize_frame(reference, target, width, height, config) -> Vec<f32>

// Apply to image in-place
map.apply(&mut pixels)

// Apply returning new image
map.apply_to_new(&pixels) -> Vec<f32>
```

**Performance**:
- Parallel tile statistics computation via rayon
- Row-based parallel processing for apply
- Segment-based interpolation (amortizes tile lookups)

**Test Coverage** (25 tests):
- Config validation and presets
- Uniform/gradient image statistics
- Offset and gradient correction
- Single tile and non-multiple-of-tile-size images
- In-place vs new apply consistency

---

## Global Normalization (Current)

The current implementation uses global statistics:
- Compute overall median and scale for reference frame
- Compute overall median and scale for each target frame
- Apply uniform offset and scale to entire frame

This works well for single-session data with minimal gradients but fails for:
- Multi-session data with different sky conditions
- Frames with varying light pollution gradients
- Mosaics with different overlap regions

---

## GPU Sigma Clipping Design (January 2026)

### Overview

GPU-accelerated sigma clipping for image stacking. Each GPU thread handles one pixel position across all N frames in the stack, computing statistics and performing iterative outlier rejection.

### Design Decisions

1. **Mean-based clipping** (not median): Computing exact median on GPU requires sorting each N-element stack, which is expensive. Mean-based clipping is:
   - Easily parallelizable via reduction
   - Works well with iterative clipping (outliers removed progressively)
   - Final result is mean anyway (lower noise than median for Gaussian data)

2. **Single-pass statistics**: Compute mean and variance in one pass using Welford's online algorithm to avoid numerical precision issues.

3. **Workgroup per tile**: Each workgroup processes a tile of pixels (e.g., 16×16 = 256 threads). Each thread computes statistics for one pixel stack.

4. **Frame data layout**: Frames stored sequentially in a single buffer. For N frames of WxH pixels:
   - `frame_data[frame_idx * W * H + y * W + x]` accesses pixel (x,y) of frame `frame_idx`
   - This layout enables coalesced reads when threads in a workgroup access adjacent pixels

### Data Structures

```rust
/// GPU sigma clipping configuration
#[derive(Debug, Clone, Copy)]
pub struct GpuSigmaClipConfig {
    pub sigma: f32,           // Clipping threshold (default: 2.5)
    pub max_iterations: u32,  // Max clipping iterations (default: 3)
}

/// Params buffer for the shader
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuSigmaClipParams {
    width: u32,
    height: u32,
    frame_count: u32,
    sigma: f32,
    max_iterations: u32,
    _padding: [u32; 3],  // Align to 16 bytes
}
```

### WGSL Shader Design

**Bindings:**
```wgsl
struct Params {
    width: u32,
    height: u32,
    frame_count: u32,
    sigma: f32,
    max_iterations: u32,
    _padding: vec3<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> frames: array<f32>;  // All frames concatenated
@group(0) @binding(2) var<storage, read_write> output: array<f32>;  // Result image
```

**Algorithm:**
```wgsl
@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let pixel_idx = y * params.width + x;
    let pixels_per_frame = params.width * params.height;

    // Load all values for this pixel into local array
    // Note: Max ~128 frames due to register pressure
    var values: array<f32, 128>;
    var valid: array<bool, 128>;
    var count = params.frame_count;

    for (var i = 0u; i < params.frame_count; i++) {
        values[i] = frames[i * pixels_per_frame + pixel_idx];
        valid[i] = true;
    }

    // Iterative sigma clipping
    for (var iter = 0u; iter < params.max_iterations; iter++) {
        if (count <= 2u) {
            break;
        }

        // Compute mean using only valid values
        var sum = 0.0;
        var n = 0u;
        for (var i = 0u; i < params.frame_count; i++) {
            if (valid[i]) {
                sum += values[i];
                n++;
            }
        }
        let mean = sum / f32(n);

        // Compute variance
        var sum_sq = 0.0;
        for (var i = 0u; i < params.frame_count; i++) {
            if (valid[i]) {
                let diff = values[i] - mean;
                sum_sq += diff * diff;
            }
        }
        let variance = sum_sq / f32(n);
        let std_dev = sqrt(variance);

        if (std_dev < 1e-10) {
            break;  // All values identical
        }

        // Mark outliers as invalid
        let threshold = params.sigma * std_dev;
        var clipped = 0u;
        for (var i = 0u; i < params.frame_count; i++) {
            if (valid[i] && abs(values[i] - mean) > threshold) {
                valid[i] = false;
                clipped++;
            }
        }

        if (clipped == 0u) {
            break;  // Converged
        }
        count -= clipped;
    }

    // Compute final mean of remaining values
    var final_sum = 0.0;
    var final_n = 0u;
    for (var i = 0u; i < params.frame_count; i++) {
        if (valid[i]) {
            final_sum += values[i];
            final_n++;
        }
    }

    output[pixel_idx] = final_sum / f32(max(final_n, 1u));
}
```

### Memory Layout

**Frame Buffer:**
- Size: `N × W × H × 4` bytes (N frames, W×H pixels, f32)
- Layout: Frame-major (all pixels of frame 0, then frame 1, etc.)
- Access pattern: Each thread reads N values at stride `W×H`

**Output Buffer:**
- Size: `W × H × 4` bytes
- Layout: Row-major
- Access pattern: Each thread writes one value

### Workgroup Sizing

- **Workgroup size**: 16×16 = 256 threads (good occupancy on most GPUs)
- **Dispatch**: `ceil(W/16) × ceil(H/16)` workgroups
- **Register pressure**: ~128 frames max due to local array size

### Performance Considerations

1. **Memory bandwidth**: Main bottleneck. Each pixel reads N×4 bytes from global memory.
   - For 24MP image with 50 frames: 24M × 50 × 4 = 4.8 GB reads
   - Typical GPU memory bandwidth: 400-900 GB/s
   - Expected: ~5-15ms for reads alone

2. **Compute**: Lightweight compared to memory
   - Per-pixel: O(N × iterations) operations
   - GPU can hide latency with many threads

3. **Transfer overhead**: Critical for CPU→GPU path
   - 4.8 GB over PCIe 3.0 x16 (~12 GB/s): ~400ms
   - GPU must provide >>10× speedup to overcome transfer
   - Best when data already on GPU (e.g., after GPU warping)

### API Design

```rust
/// GPU-accelerated sigma clipping stacker
pub struct GpuSigmaClipper {
    ctx: ProcessingContext,
    pipeline: GpuSigmaClipPipeline,
}

impl GpuSigmaClipper {
    pub fn new() -> Self;

    /// Stack frames using GPU sigma clipping.
    /// Frames must all have same dimensions.
    pub fn stack(
        &mut self,
        frames: &[&[f32]],  // Slice of frame data
        width: usize,
        height: usize,
        config: &GpuSigmaClipConfig,
    ) -> Vec<f32>;

    /// Stack frames already on GPU (from GpuImage).
    /// More efficient when data is already on GPU.
    pub fn stack_gpu(
        &mut self,
        frames: &[&GpuImage],
        config: &GpuSigmaClipConfig,
    ) -> GpuImage;
}
```

### Integration with Existing Code

**Automatic backend selection:**
```rust
pub fn stack_sigma_clipped_from_paths<P: AsRef<Path> + Sync>(
    paths: &[P],
    frame_type: FrameType,
    config: &SigmaClippedConfig,
    progress: ProgressCallback,
) -> Result<AstroImage, Error> {
    // Use GPU if:
    // 1. GPU is available
    // 2. Frame count >= threshold (e.g., 30)
    // 3. Image size >= threshold (e.g., 4K)
    if config.use_gpu && should_use_gpu(paths.len(), expected_size) {
        return stack_sigma_clipped_gpu(paths, frame_type, config, progress);
    }

    // Fall back to CPU implementation
    let cache = ImageCache::from_paths(paths, &config.cache, frame_type, progress)?;
    // ... existing CPU code ...
}
```

### Fallback Strategy

1. **Small stacks (<20 frames)**: CPU always faster (GPU transfer overhead)
2. **Small images (<1K×1K)**: CPU likely faster
3. **No GPU available**: Automatic CPU fallback
4. **GPU out of memory**: Fall back to CPU with warning

### Testing Strategy

1. **Correctness**: Compare GPU results to CPU reference within tolerance (±1e-4)
2. **Edge cases**:
   - Single frame (no clipping needed)
   - Two frames (minimal clipping possible)
   - All identical values (std_dev = 0)
   - All outliers except one
3. **Performance**: Benchmark vs CPU at various frame counts and image sizes

### Implementation Status (COMPLETE - 2026-01-27)

**Files Implemented:**
- `src/stacking/gpu/mod.rs` - Module definition and re-exports
- `src/stacking/gpu/sigma_clip.rs` - GpuSigmaClipper implementation
- `src/stacking/gpu/sigma_clip.wgsl` - WGSL compute shader
- `src/stacking/gpu/pipeline.rs` - Pipeline setup using GpuPipeline trait
- `src/stacking/gpu/bench.rs` - GPU vs CPU benchmarks (feature = "bench")

**API:**
```rust
// Configuration
GpuSigmaClipConfig::new(sigma: 2.5, max_iterations: 3)
GpuSigmaClipConfig::default()  // sigma=2.5, max_iterations=3

// Stacking
let mut clipper = GpuSigmaClipper::new();
if clipper.gpu_available() {
    let result = clipper.stack(&frames, width, height, &config);
}
```

**Tests (11 passing):**
- `test_config_default`, `test_config_new`
- `test_config_zero_sigma_panics`, `test_config_zero_iterations_panics`
- `test_stack_identical_values`, `test_stack_with_outlier`
- `test_stack_two_frames_no_clipping`, `test_stack_single_frame`
- `test_stack_large_image`, `test_stack_empty_frames_panics`
- `test_stack_too_many_frames_panics`

**Exports:**
- `lumos::GpuSigmaClipper`, `lumos::GpuSigmaClipConfig`, `lumos::GpuSigmaClipPipeline`
- `lumos::MAX_GPU_FRAMES` (128)

**Benchmark:**
```bash
cargo bench -p lumos --features bench --bench stack_gpu_sigma_clip
```
Benchmarks GPU vs CPU for various image sizes (256x256 to 2048x2048) and frame counts (10-50).

---

## Batch Processing Pipeline (IMPLEMENTED - 2026-01-27)

### Overview

Multi-batch GPU processing with overlapped compute/transfer for stacking large numbers of frames (>128, the GPU shader limit). Frames are processed in batches with partial results combined using weighted mean.

### Architecture

```text
┌─────────────────────────────────────────────────────────────────┐
│                    Batch Processing Pipeline                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   Frame Count > 128                                              │
│        ↓                                                         │
│   Split into batches (≤128 frames each)                          │
│        ↓                                                         │
│   For each batch:                                                │
│     1. Upload frame data to GPU                                  │
│     2. Run sigma clipping compute shader                         │
│     3. Read back batch result                                    │
│        ↓                                                         │
│   Combine batch results (weighted mean by frame count)           │
│        ↓                                                         │
│   Final stacked image                                            │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Files

- `src/stacking/gpu/batch_pipeline.rs` - BatchPipeline and BatchPipelineConfig

### API

```rust
// Configuration
let config = BatchPipelineConfig::default();  // sigma=2.5, max_iter=3, batch=128
let config = BatchPipelineConfig::with_sigma_clip(2.0, 3);
let config = config.batch_size(64);  // smaller batches for more overlap
let config = config.triple_buffer(); // 3 buffer slots instead of 2

// Stacking pre-loaded frames
let mut pipeline = BatchPipeline::new(config);
let result = pipeline.stack(&frames, width, height);

// Stacking from file paths (with parallel I/O)
let result = pipeline.stack_from_paths(&paths, width, height)?;
```

### Batch Result Combination

When processing >128 frames, each batch produces a partial result. These are combined using weighted mean based on frame count:

```
combined[pixel] = Σ(batch_result[pixel] × batch_frame_count) / total_frames
```

This ensures batches with more frames contribute proportionally more to the final result.

### Tests (18 passing)

- Config tests: `test_config_default`, `test_config_with_sigma_clip`, `test_config_batch_size`, `test_config_triple_buffer`
- Config panic tests: `test_config_batch_size_zero_panics`, `test_config_batch_size_too_large_panics`
- Stack tests: `test_stack_single_batch`, `test_stack_multi_batch`, `test_stack_single_batch_with_outlier`, `test_stack_with_outlier_multi_batch`
- Combine tests: `test_combine_batch_results_single`, `test_combine_batch_results_equal_weights`, `test_combine_batch_results_weighted`
- Async tests: `test_stack_async_identical_values`, `test_stack_async_with_outlier`, `test_stack_async_matches_sync`, `test_stack_async_triple_buffer`, `test_combine_batch_results_with_weights`

### Exports

- `lumos::BatchPipeline`, `lumos::BatchPipelineConfig`

---

## Async GPU Operations (IMPLEMENTED - 2026-01-27)

### Overview

True overlapped compute/transfer operations using double-buffering with proper GPU synchronization. The `stack_async()` method provides efficient multi-batch processing where:
- GPU computes on batch N while batch N-1's result is being read back
- Different buffer slots are used to avoid resource conflicts

### Architecture

```text
Timeline for Double-Buffering:

Time →
Slot A: [Upload B0] [Compute B0] [Readback B0]           [Upload B2] [Compute B2] ...
Slot B:             [Upload B1]  [Compute B1] [Readback B1]          [Upload B3] ...
```

### Key Types

```rust
/// Pending async readback operation
struct PendingReadback {
    staging_buffer: wgpu::Buffer,  // Owned buffer for async mapping
    pixel_count: usize,
    ready: Arc<AtomicBool>,        // Completion flag set by callback
    batch_idx: usize,
    frame_count: usize,
}
```

### Synchronization Mechanism

1. **Callback-based completion**: `map_async` callback sets `AtomicBool` when mapping completes
2. **Non-blocking poll**: `device.poll(PollType::Poll)` checks progress without blocking
3. **Ordered collection**: Results sorted by batch index before weighted combination

### API

```rust
let mut pipeline = BatchPipeline::new(config);

// Async stacking with overlapped compute/transfer
let result = pipeline.stack_async(&frames, width, height);

// For small batch counts (≤2), falls back to sync version
// For larger batch counts, uses true double-buffering
```

### Performance Characteristics

- **Best for**: Large frame counts (>2× batch_size) with expensive compute
- **Overlap benefit**: Readback of batch N-1 overlaps with compute of batch N
- **Memory tradeoff**: Each pending readback needs its own staging buffer

### Implementation Notes

- Each pending readback owns its staging buffer (moved ownership for callback safety)
- `AtomicBool` with `Acquire/Release` ordering for cross-thread synchronization
- Fallback to sync version for small batch counts (overlap overhead not worth it)

---

## Comet/Asteroid Stacking Research (2026-01-27)

### Problem Statement

Comets and asteroids move relative to stars during an imaging session. When stacking:
- **Aligning on stars**: Comet/asteroid becomes blurred or trails
- **Aligning on comet**: Stars become trailed

The goal is to produce a composite image with both sharp stars AND a sharp comet/asteroid.

### Dual-Stack Approach (Industry Standard)

**Sources:**
- [Siril Comet Tutorial](https://siril.org/tutorials/comet/)
- [DeepSkyStacker Comet Stacking](http://deepskystacker.free.fr/english/technical.htm)
- [PixInsight CometAlignment](https://pixinsight.com/forum/index.php?threads/pcl-comet-alignment-module.3838/)
- [DeepSkyWorkflows Complete Comet Guide](https://deepskyworkflows.com/how-do-i-stack-comets-in-pixinsight/)

**Workflow:**

1. **Create Star-Aligned Stack**
   - Register frames using standard star matching (triangle matching, RANSAC)
   - Stack using sigma clipping or kappa-sigma clipping
   - Result: Sharp stars, moving object appears as trail/blur

2. **Create Comet-Aligned Stack**
   - User provides comet position in first frame (x₁, y₁) and last frame (x₂, y₂)
   - Calculate velocity vector from timestamps: `v = (x₂-x₁, y₂-y₁) / (t₂-t₁)`
   - For each intermediate frame, interpolate position: `pos(t) = (x₁, y₁) + v × (t - t₁)`
   - Register each frame with additional translation to center comet
   - Stack using sigma clipping (rejects star trails as outliers)
   - Result: Sharp comet, stars appear as trails

3. **Composite Final Image**
   - Combine using lighten blending: `max(star_stack, comet_stack)` per pixel
   - Or additive: `star_stack + comet_stack` (requires removing background from comet stack)
   - Or layer masking: Use comet region from comet stack, stars from star stack

### Position Interpolation Algorithm

**Linear interpolation** (most common, works for short sessions):
```
t_normalized = (frame_timestamp - t_first) / (t_last - t_first)
x_comet = x_first + t_normalized * (x_last - x_first)
y_comet = y_first + t_normalized * (y_last - y_first)
```

**Timestamp handling:**
- Primary: `DATE-OBS` FITS keyword (observation start time)
- Fallback: `FileLastModificationDate = DATE-OBS + Exposure` (for RAW files)
- Units: Usually hours or fractional days (MJD)

**PSF-based refinement** (PixInsight 2.0+):
- Calculate nucleus position using optimal PSF fitting
- Allows non-linear paths (for curved comet trajectories)
- More accurate than manual marking

### Transform Computation

For comet-aligned registration:
```
transform_comet = transform_stars ∘ Translation(-dx_comet, -dy_comet)
```

Where:
- `transform_stars` = standard star-based registration transform
- `dx_comet, dy_comet` = comet position offset from frame center

### Pixel Rejection for Trail Removal

**Kappa-sigma clipping** effectively removes star trails from comet stack:
- Star trails appear as bright outliers at each pixel
- With >10 frames, trails don't overlap enough to survive rejection
- Lower kappa values (2.0-2.5) remove more trail pixels

**Winsorized sigma clipping** can help preserve comet detail while removing trails.

### Composite Methods

1. **Lighten blend** (simplest):
   ```
   output[pixel] = max(star_stack[pixel], comet_stack[pixel])
   ```
   Works when backgrounds are dark and matched.

2. **Additive blend with background subtraction**:
   ```
   comet_starless = comet_stack - background(comet_stack)
   output = star_stack + comet_starless
   ```
   Better preserves faint structures.

3. **Layer masking**:
   - Create mask around comet region (manual or auto-detected)
   - Blend: `output = star_stack * (1-mask) + comet_stack * mask`
   - Most control but requires mask generation.

### Proposed API Design

```rust
/// Comet/asteroid position at a specific timestamp
#[derive(Debug, Clone, Copy)]
pub struct ObjectPosition {
    pub x: f64,
    pub y: f64,
    pub timestamp: f64,  // MJD or seconds since epoch
}

/// Configuration for comet stacking
#[derive(Debug, Clone)]
pub struct CometStackConfig {
    /// Position at start of sequence
    pub pos_start: ObjectPosition,
    /// Position at end of sequence
    pub pos_end: ObjectPosition,
    /// Rejection method for star trail removal
    pub rejection: RejectionMethod,
    /// Normalization method
    pub normalization: NormalizationMethod,
    /// How to combine star and comet stacks
    pub composite_method: CompositeMethod,
}

#[derive(Debug, Clone, Copy)]
pub enum CompositeMethod {
    /// Max of each pixel (lighten blend)
    Lighten,
    /// Additive with automatic background subtraction
    Additive,
    /// Return both stacks separately for manual compositing
    Separate,
}

/// Result of comet stacking
#[derive(Debug)]
pub struct CometStackResult {
    /// Star-aligned stack (sharp stars, blurred comet)
    pub star_stack: Vec<f32>,
    /// Comet-aligned stack (sharp comet, star trails rejected)
    pub comet_stack: Vec<f32>,
    /// Composite image (if not Separate)
    pub composite: Option<Vec<f32>>,
    /// Computed comet velocity (pixels/hour)
    pub velocity: (f64, f64),
}

// Main function
pub fn stack_comet<P: AsRef<Path>>(
    paths: &[P],
    config: &CometStackConfig,
) -> Result<CometStackResult, Error>;

// Interpolate position for a given timestamp
pub fn interpolate_position(
    pos_start: &ObjectPosition,
    pos_end: &ObjectPosition,
    timestamp: f64,
) -> (f64, f64);
```

### Implementation Steps

1. **Research** ✓ - Completed 2026-01-27
2. **API Design** - Define types and function signatures
3. **Position interpolation** - Linear interpolation from timestamps
4. **Comet-aligned registration** - Modify transforms with comet offset
5. **Dual stacking** - Run star-aligned and comet-aligned stacks
6. **Composite generation** - Implement blend modes
7. **Testing** - Synthetic moving object tests
