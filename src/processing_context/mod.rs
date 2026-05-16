#[cfg(feature = "wgpu")]
pub(crate) mod gpu_context;
pub(crate) mod image_buffer;
#[cfg(all(test, feature = "wgpu"))]
mod tests;

#[cfg(feature = "wgpu")]
use crate::gpu::Gpu;
#[cfg(feature = "wgpu")]
use crate::processing_context::gpu_context::GpuContext;

/// Processing context that manages GPU resources and cached pipelines.
///
/// This is the main entry point for image processing operations.
#[derive(Debug)]
pub struct ProcessingContext {
    #[cfg(feature = "wgpu")]
    pub(crate) gpu_context: Option<GpuContext>,
}

impl ProcessingContext {
    /// Creates a new ProcessingContext, attempting to initialize GPU.
    /// Falls back to CPU-only if GPU is unavailable.
    #[allow(clippy::new_without_default)] // intentionally no Default to force explicit cpu_only() vs new()
    pub fn new() -> Self {
        #[cfg(feature = "wgpu")]
        {
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
        #[cfg(not(feature = "wgpu"))]
        {
            Self {}
        }
    }

    /// Creates a CPU-only ProcessingContext (no GPU).
    pub fn cpu_only() -> Self {
        Self {
            #[cfg(feature = "wgpu")]
            gpu_context: None,
        }
    }

    /// Returns true if GPU is available.
    pub fn has_gpu(&self) -> bool {
        #[cfg(feature = "wgpu")]
        {
            self.gpu_context.is_some()
        }
        #[cfg(not(feature = "wgpu"))]
        {
            false
        }
    }

    /// Returns a reference to the GPU context if available.
    #[cfg(feature = "wgpu")]
    pub fn gpu(&self) -> Option<&Gpu> {
        self.gpu_context.as_ref().map(|p| &p.gpu)
    }

    /// Returns a mutable reference to the GPU processing context.
    /// Returns None if no GPU is available.
    #[cfg(feature = "wgpu")]
    pub fn gpu_context(&mut self) -> Option<&mut GpuContext> {
        self.gpu_context.as_mut()
    }
}
