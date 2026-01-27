//! GPU-accelerated stacking operations.
//!
//! This module provides GPU acceleration for compute-intensive stacking operations.

#[cfg(feature = "bench")]
pub mod bench;

mod pipeline;
mod sigma_clip;

pub use pipeline::GpuSigmaClipPipeline;
pub use sigma_clip::{GpuSigmaClipConfig, GpuSigmaClipper, MAX_GPU_FRAMES};
