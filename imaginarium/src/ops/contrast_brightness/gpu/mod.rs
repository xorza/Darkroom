use bytemuck::{Pod, Zeroable};

use crate::gpu::Gpu;
use crate::gpu::compute_kernel::{KernelJob, OutputStart};
use crate::gpu::gpu_image::GpuImage;
use crate::gpu::packed_layout::PackedLayout;
use crate::ops::contrast_brightness::ContrastBrightness;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;
use crate::ops::contrast_brightness::pipeline;

/// The GPU path behind [`ContrastBrightness::apply_gpu`].
pub(super) fn apply(params: ContrastBrightness, gpu: &Gpu, image: &mut GpuImage) {
    let affine = ChannelAffine::new(params, image.desc.color_format());
    let uniform = Params {
        scale: affine.scale,
        offset: affine.offset,
        max: affine.max,
        _pad: 0,
        layout: PackedLayout::new(image.desc),
    };
    pipeline::kernel(gpu).run(
        gpu,
        KernelJob {
            params: bytemuck::bytes_of(&uniform),
            buffers: &[image.write_buffer()],
            invocations: uniform.layout.words(),
            output: OutputStart::AsIs,
        },
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
