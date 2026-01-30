//! GPU-accelerated stacking operations.
//!
//! This module provides GPU acceleration for compute-intensive stacking operations.
//!
//! ## Components
//!
//! - [`GpuSigmaClipper`]: GPU sigma clipping for single-batch operations (≤128 frames)
//! - [`BatchPipeline`]: Multi-batch processing with overlapped compute/transfer
//!
//! ## Usage
//!
//! For small frame counts (≤128), use `GpuSigmaClipper` directly:
//!
//! ```ignore
//! let mut clipper = GpuSigmaClipper::new();
//! let result = clipper.stack(&frames, width, height, &config);
//! ```
//!
//! For large frame counts or when loading from disk, use `BatchPipeline`:
//!
//! ```ignore
//! let mut pipeline = BatchPipeline::new(BatchPipelineConfig::default());
//! let result = pipeline.stack_from_paths(&paths, width, height)?;
//! ```

mod batch_pipeline;
mod pipeline;
mod sigma_clip;

pub use batch_pipeline::{BatchPipeline, BatchPipelineConfig};
pub use pipeline::GpuSigmaClipPipeline;
pub use sigma_clip::{GpuSigmaClipConfig, GpuSigmaClipper, MAX_GPU_FRAMES};
