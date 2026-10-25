use crate::gpu::Gpu;
use crate::gpu::compute_kernel::ComputeKernel;
use crate::gpu::context::GpuPipeline;

/// Cached GPU pipeline for contrast/brightness operations.
/// Create once and reuse for multiple executions.
#[derive(Debug)]
pub struct GpuContrastBrightnessPipeline {
    pub(super) kernel: ComputeKernel,
}

impl GpuContrastBrightnessPipeline {
    /// Compiles the contrast/brightness shader for `ctx`'s device.
    pub fn new(ctx: &Gpu) -> Self {
        Self {
            kernel: ComputeKernel::new(
                ctx,
                "contrast_brightness",
                include_str!("contrast_brightness.wgsl"),
                2,
            ),
        }
    }
}

impl GpuPipeline for GpuContrastBrightnessPipeline {}
