use crate::gpu::Gpu;
use crate::gpu::compute_kernel::ComputeKernel;
use crate::gpu::context::GpuPipeline;

/// Cached GPU pipeline for affine transforms.
/// Create once and reuse for multiple executions.
#[derive(Debug)]
pub struct GpuTransformPipeline {
    pub(super) kernel: ComputeKernel,
}

impl GpuTransformPipeline {
    /// Compiles the transform shader for `ctx`'s device.
    pub fn new(ctx: &Gpu) -> Self {
        Self {
            kernel: ComputeKernel::new(ctx, "transform", include_str!("shader.wgsl"), 2),
        }
    }
}

impl GpuPipeline for GpuTransformPipeline {}
