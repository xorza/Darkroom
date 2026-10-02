use crate::gpu::Gpu;
use crate::gpu::compute_kernel::ComputeKernel;
use crate::gpu::context::GpuPipeline;

/// Cached GPU pipeline for blend operations.
/// Create once and reuse for multiple executions.
#[derive(Debug)]
pub struct GpuBlendPipeline {
    pub(super) kernel: ComputeKernel,
}

impl GpuBlendPipeline {
    /// Compiles the blend shader for `ctx`'s device.
    pub fn new(ctx: &Gpu) -> Self {
        Self {
            kernel: ComputeKernel::new(ctx, "blend", include_str!("blend.wgsl"), 3),
        }
    }
}

impl GpuPipeline for GpuBlendPipeline {}
