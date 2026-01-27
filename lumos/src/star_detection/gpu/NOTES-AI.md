# GPU Star Detection - Implementation Notes (AI)

## Overview

GPU-accelerated threshold detection for the star detection pipeline. Uses a hybrid GPU/CPU approach for optimal performance on sparse astronomical images.

## Architecture

### Design Decision: Hybrid GPU/CPU Approach

After researching GPU connected component labeling (CCL) algorithms including:
- Block-Based Union-Find (BUF) - [Allegretti et al. 2019](https://iris.unimore.it/retrieve/handle/11380/1179642/227505/2019__ICIAP_A_Block_Based_Union_Find_Algorithm_to_Label_Connected_Components_on_GPUs.pdf)
- [Optimized Union-Find for GPU CCL](https://arxiv.org/abs/1708.08180)

We chose a **hybrid approach** because:

1. **Sparse images**: Astronomical images typically have <5% foreground pixels. GPU CCL algorithms are optimized for dense images (>50% foreground) and actually perform worse than CPU on sparse data.

2. **Memory transfer cost**: Full GPU CCL would require keeping all intermediate data on GPU. The boolean mask transfer (1 byte/pixel) is negligible compared to processing cost.

3. **Complexity vs. benefit**: GPU CCL algorithms require complex multi-kernel approaches (initialization, merge, compression passes) with atomic operations. For sparse images, CPU union-find with path compression is competitive.

4. **Current bottleneck**: Profiling shows the threshold mask creation is the main per-pixel operation. CCL on <5% of pixels is fast.

### Pipeline

```
┌─────────────────────────────────────────────────────────────────────┐
│                    GPU Star Detection Pipeline                       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────┐│
│  │   GPU: Create   │──▶│  GPU: Dilate    │──▶│  CPU: Connected     ││
│  │ Threshold Mask  │   │     Mask        │   │  Component Labeling ││
│  └─────────────────┘   └─────────────────┘   └─────────────────────┘│
│         ▲                     │                        │             │
│         │                     │                        ▼             │
│  ┌──────┴──────┐        ┌────▴────┐          ┌─────────────────────┐│
│  │ Image +     │        │ Binary  │          │ Label Map +         ││
│  │ Background  │        │ Mask    │          │ Candidates          ││
│  │ + Noise     │        │         │          │                     ││
│  └─────────────┘        └─────────┘          └─────────────────────┘│
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

## GPU Kernel Design

### 1. Threshold Mask Creation Kernel (`threshold_mask.wgsl`)

**Purpose**: Create binary mask of pixels above detection threshold.

**Operation per pixel**:
```
threshold = background[i] + sigma_threshold * noise[i]
mask[i] = pixels[i] > threshold ? 1 : 0
```

**Workgroup size**: 16x16 = 256 threads (optimal for image processing)

**Inputs**:
- `pixels`: f32 image data
- `background`: f32 per-pixel background estimates
- `noise`: f32 per-pixel noise estimates
- `sigma_threshold`: f32 uniform

**Output**:
- `mask`: u32 packed bits (32 pixels per u32 for efficiency)

**Optimization**: Pack 32 mask bits into each u32 to reduce memory bandwidth by 32x.

### 2. Mask Dilation Kernel (`dilate_mask.wgsl`)

**Purpose**: Dilate binary mask by radius R to connect fragmented detections.

**Operation**: Each output pixel is 1 if any pixel within radius R in the input is 1.

**Workgroup size**: 16x16 with shared memory tiling

**Algorithm**:
1. Load (16+2R) × (16+2R) tile into shared memory (with halo)
2. Each thread checks R×R neighborhood
3. Output 1 if any neighbor is 1

**Inputs**:
- `mask_in`: u32 packed bits
- `radius`: u32 uniform

**Output**:
- `mask_out`: u32 packed bits

## Data Structures

### GpuThresholdConfig

```rust
#[derive(Debug, Clone, Copy)]
pub struct GpuThresholdConfig {
    /// Detection threshold in sigma above background.
    pub sigma_threshold: f32,
    /// Dilation radius in pixels.
    pub dilation_radius: u32,
}
```

### GpuThresholdDetector

```rust
#[derive(Debug)]
pub struct GpuThresholdDetector {
    ctx: ProcessingContext,
}

impl GpuThresholdDetector {
    /// Create threshold mask on GPU.
    pub fn create_mask(
        &mut self,
        pixels: &[f32],
        background: &BackgroundMap,
        config: &GpuThresholdConfig,
    ) -> Vec<bool>;
}
```

## Public API

### detect_stars_gpu

Full star detection using GPU for threshold mask creation:

```rust
pub fn detect_stars_gpu(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate>;
```

### detect_stars_gpu_with_detector

Same as above but allows reusing the GPU context for better performance:

```rust
pub fn detect_stars_gpu_with_detector(
    detector: &mut GpuThresholdDetector,
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate>;
```

## Performance Expectations

For a 4096×4096 image:
- **CPU threshold mask**: ~15ms (current SIMD implementation)
- **GPU threshold mask**: ~2-3ms (expected with memory transfer)
- **CPU dilation (R=1)**: ~8ms
- **GPU dilation (R=1)**: ~1ms (expected)
- **CPU CCL (5% foreground)**: ~5ms

**Expected total speedup**: ~40% for threshold + dilation phases.

## Implementation Status

| Component | Status |
|-----------|--------|
| Design | Complete |
| threshold_mask.wgsl | Complete |
| dilate_mask.wgsl | Complete |
| pipeline.rs | Complete |
| GpuThresholdDetector | Complete |
| detect_stars_gpu | Complete |
| detect_stars_gpu_with_detector | Complete |
| Integration tests | Complete (15 tests) |
| Benchmarks | Ready (bench feature has pre-existing issues) |

## Usage

For batch processing (best performance):
```rust
let mut detector = GpuThresholdDetector::new();
if detector.gpu_available() {
    for image in images {
        let candidates = detect_stars_gpu_with_detector(
            &mut detector, &pixels, width, height, &background, &config
        );
        // Process candidates...
    }
}
```

For single-use:
```rust
let candidates = detect_stars_gpu(&pixels, width, height, &background, &config);
```

## Future Considerations

### Full GPU CCL (if needed later)

If profiling shows CCL becomes a bottleneck for dense regions (unlikely for astronomical images), consider implementing Block-Based Union-Find:

1. **Init kernel**: Assign each foreground pixel its own label
2. **Local merge kernel**: Union-find within 2x2 blocks
3. **Boundary kernel**: Analyze block boundaries, create equivalence
4. **Global merge kernel**: Merge equivalent labels with atomics
5. **Compression kernel**: Flatten all labels to roots

This would add significant complexity (~500 lines of WGSL + Rust) for marginal gains on sparse images.

## References

- [Block-Based Union-Find Algorithm for GPU CCL](https://www.federicobolelli.it/media/publications/pdfs/2019iciap_labeling.pdf)
- [Optimized Union-Find for GPU CCL](https://arxiv.org/abs/1708.08180)
- [NVIDIA GPU CCL Presentation](https://developer.download.nvidia.com/video/gputechconf/gtc/2019/presentation/s9111-a-new-direct-connected-component-labeling-and-analysis-algorithm-for-gpus.pdf)
