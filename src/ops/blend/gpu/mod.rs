use bytemuck::{Pod, Zeroable};

use crate::gpu::Gpu;
use crate::gpu::gpu_image::GpuImage;
use crate::gpu::packed_layout::PackedLayout;
use crate::ops::blend::Blend;
use crate::ops::blend::pipeline::GpuBlendPipeline;

/// The GPU path behind [`Blend::apply_gpu`].
pub(super) fn apply(
    params: Blend,
    ctx: &Gpu,
    pipeline: &GpuBlendPipeline,
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
        _pad: [0; 2],
        layout: PackedLayout::new(src.desc),
    };
    pipeline.kernel.run(
        ctx,
        bytemuck::bytes_of(&uniform),
        &[src.read_buffer(), dst.read_buffer(), output.write_buffer()],
        uniform.layout.words(),
        false,
    );
}

/// The shader's `Params`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Params {
    mode: u32,
    alpha: f32,
    _pad: [u32; 2],
    layout: PackedLayout,
}

#[cfg(test)]
mod tests;
