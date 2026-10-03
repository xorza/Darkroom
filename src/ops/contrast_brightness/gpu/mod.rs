use bytemuck::{Pod, Zeroable};

use crate::gpu::Gpu;
use crate::gpu::gpu_image::GpuImage;
use crate::gpu::packed_layout::PackedLayout;
use crate::ops::contrast_brightness::ContrastBrightness;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;
use crate::ops::contrast_brightness::pipeline::GpuContrastBrightnessPipeline;

/// The GPU path behind [`ContrastBrightness::apply_gpu`].
pub(super) fn apply(
    params: ContrastBrightness,
    ctx: &Gpu,
    pipeline: &GpuContrastBrightnessPipeline,
    input: &GpuImage,
    output: &mut GpuImage,
) {
    input.desc.assert_same(output.desc, "input/output");
    let affine = ChannelAffine::new(params, input.desc.color_format);
    let uniform = Params {
        scale: affine.scale,
        offset: affine.offset,
        max: affine.max,
        _pad: 0,
        layout: PackedLayout::new(input.desc),
    };
    pipeline.kernel.run(
        ctx,
        bytemuck::bytes_of(&uniform),
        &[input.read_buffer(), output.write_buffer()],
        uniform.layout.words(),
        false,
    );
}

/// The shader's `Params`: the CPU's affine, value for value.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Params {
    scale: f32,
    offset: f32,
    max: f32,
    _pad: u32,
    layout: PackedLayout,
}

#[cfg(test)]
mod tests;
