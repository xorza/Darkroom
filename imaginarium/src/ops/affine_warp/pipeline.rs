//! The op's GPU kernel, compiled once per device.

use crate::gpu::compute_kernel::ComputeKernel;
use crate::gpu::{Gpu, GpuOp};

/// The op's kernel on `gpu`, compiled on its first use there.
pub(super) fn kernel(gpu: &Gpu) -> &ComputeKernel {
    gpu.kernel(GpuOp::AffineWarp, |gpu| {
        ComputeKernel::new(gpu, "affine_warp", include_str!("shader.wgsl"), 2)
    })
}
