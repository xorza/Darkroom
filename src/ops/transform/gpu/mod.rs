use bytemuck::{Pod, Zeroable};

use crate::gpu::Gpu;
use crate::gpu::gpu_image::GpuImage;
use crate::ops::transform::pipeline::GpuTransformPipeline;
use crate::ops::transform::{FilterMode, Transform};

/// The GPU path behind [`Transform::apply_gpu`].
pub(super) fn apply(
    params: &Transform,
    ctx: &Gpu,
    pipeline: &GpuTransformPipeline,
    input: &GpuImage,
    output: &mut GpuImage,
) {
    let (input_desc, output_desc) = (input.desc, output.desc);
    assert_eq!(
        input_desc.color_format, output_desc.color_format,
        "input/output color format mismatch"
    );
    let format = input_desc.color_format;
    let inv = params.inverse();
    let extent = |value: usize| u32::try_from(value).expect("a GpuImage extent fits a u32");

    let uniform = Params {
        inv_matrix: inv.matrix2.to_cols_array(),
        inv_translation: inv.translation.to_array(),
        input_size: [extent(input_desc.width), extent(input_desc.height)],
        output_size: [extent(output_desc.width), extent(output_desc.height)],
        channels: extent(format.channel_count.count()),
        elem_size: extent(format.sample_type.size()),
        filter_mode: match params.filter {
            FilterMode::Nearest => 0,
            FilterMode::Bilinear => 1,
        },
        _pad: 0,
    };
    pipeline.kernel.run(
        ctx,
        bytemuck::bytes_of(&uniform),
        &[input.read_buffer(), output.write_buffer()],
        extent(output_desc.width * output_desc.height),
        // Sub-word elements are ORed into the output.
        format.sample_type.size() < 4,
    );
}

/// The shader's `Params`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Params {
    /// Column-major, as WGSL's `mat2x2`.
    inv_matrix: [f32; 4],
    inv_translation: [f32; 2],
    input_size: [u32; 2],
    output_size: [u32; 2],
    channels: u32,
    elem_size: u32,
    filter_mode: u32,
    _pad: u32,
}

#[cfg(test)]
mod tests;
