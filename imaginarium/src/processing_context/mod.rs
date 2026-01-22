mod gpu_context;
mod image_buffer;
#[cfg(test)]
mod tests;

pub use gpu_context::{GpuContext, GpuPipeline};
pub use image_buffer::{ImageBuffer, Storage};

use crate::prelude::*;

/// Processing context that manages GPU resources and cached pipelines.
///
/// This is the main entry point for image processing operations.
#[derive(Debug)]
pub struct ProcessingContext {
    gpu_context: Option<GpuContext>,
}

impl ProcessingContext {
    /// Creates a new ProcessingContext, attempting to initialize GPU.
    /// Falls back to CPU-only if GPU is unavailable.
    pub fn new() -> Self {
        match Gpu::new() {
            Ok(ctx) => Self {
                gpu_context: Some(GpuContext::new(ctx)),
            },
            Err(e) => {
                tracing::warn!("GPU initialization failed, falling back to CPU: {}", e);
                Self { gpu_context: None }
            }
        }
    }

    /// Creates a CPU-only ProcessingContext (no GPU).
    pub fn cpu_only() -> Self {
        Self { gpu_context: None }
    }

    /// Creates a ProcessingContext with the given GPU context.
    pub fn with_gpu(gpu_context: GpuContext) -> Self {
        Self {
            gpu_context: Some(gpu_context),
        }
    }

    /// Returns true if GPU is available.
    pub fn has_gpu(&self) -> bool {
        self.gpu_context.is_some()
    }

    /// Returns a reference to the GPU context if available.
    pub fn gpu(&self) -> Option<&Gpu> {
        self.gpu_context.as_ref().map(|p| p.gpu())
    }

    /// Returns a mutable reference to the GPU processing context.
    /// Returns None if no GPU is available.
    pub fn gpu_context(&mut self) -> Option<&mut GpuContext> {
        self.gpu_context.as_mut()
    }
}

impl Default for ProcessingContext {
    fn default() -> Self {
        Self::new()
    }
}
