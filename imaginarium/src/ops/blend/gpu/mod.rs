use bytemuck::{Pod, Zeroable};

use crate::gpu::Gpu;
use crate::gpu::compute_kernel::{KernelJob, OutputStart};
use crate::gpu::gpu_image::GpuImage;
use crate::gpu::packed_layout::PackedLayout;
use crate::ops::blend::Blend;
use crate::ops::blend::pipeline;

/// The GPU path behind [`Blend::apply_gpu`].
pub(super) fn apply(
    params: Blend,
    gpu: &Gpu,
    src: &GpuImage,
    dst: &GpuImage,
    output: &mut GpuImage,
) {
    src.desc.assert_same(dst.desc, "src/dst");
    src.desc.assert_same(output.desc, "src/output");

    let uniform = Params {
        // `BlendMode` is `#[repr(u8)]`; the shader's mode contract depends on this order.
        mode: params.mode as u32,
        alpha: params.alpha,
        clamp: params.clamp.shader_code(),
        _pad: 0,
        layout: PackedLayout::new(src.desc),
    };
    pipeline::kernel(gpu).run(
        gpu,
        KernelJob {
            params: bytemuck::bytes_of(&uniform),
            buffers: &[src.read_buffer(), dst.read_buffer(), output.write_buffer()],
            invocations: uniform.layout.words(),
            output: OutputStart::AsIs,
        },
    );
}

/// The shader's `Params`, the clamp by its shader code.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Params {
    mode: u32,
    alpha: f32,
    clamp: u32,
    _pad: u32,
    layout: PackedLayout,
}

#[cfg(test)]
mod tests;
