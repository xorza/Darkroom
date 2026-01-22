use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::pipeline::GpuTransformPipeline;
use super::{FilterMode, Transform};
use crate::gpu::GpuImage;
use crate::prelude::*;

// Format types matching shader constants
const FORMAT_GRAY_U8: u32 = 0;
const FORMAT_GRAY_ALPHA_U8: u32 = 1;
const FORMAT_RGB_U8: u32 = 2;
const FORMAT_RGBA_U8: u32 = 3;
const FORMAT_GRAY_F32: u32 = 4;
const FORMAT_GRAY_ALPHA_F32: u32 = 5;
const FORMAT_RGB_F32: u32 = 6;
const FORMAT_RGBA_F32: u32 = 7;
const FORMAT_GRAY_U16: u32 = 8;
const FORMAT_GRAY_ALPHA_U16: u32 = 9;
const FORMAT_RGB_U16: u32 = 10;
const FORMAT_RGBA_U16: u32 = 11;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Params {
    // Inverse transform matrix (2x2, column-major for WGSL mat2x2)
    inv_matrix: [f32; 4],
    // Inverse translation
    inv_translation: [f32; 2],
    // Input dimensions
    input_width: u32,
    input_height: u32,
    // Output dimensions
    output_width: u32,
    output_height: u32,
    // Input/output strides in bytes
    input_stride: u32,
    output_stride: u32,
    // Filter mode: 0 = Nearest, 1 = Bilinear
    filter_mode: u32,
    // Format type
    format_type: u32,
}

fn get_format_type(format: ColorFormat) -> u32 {
    match (
        format.channel_count,
        format.channel_size,
        format.channel_type,
    ) {
        (ChannelCount::Gray, ChannelSize::_8bit, ChannelType::UInt) => FORMAT_GRAY_U8,
        (ChannelCount::GrayAlpha, ChannelSize::_8bit, ChannelType::UInt) => FORMAT_GRAY_ALPHA_U8,
        (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::UInt) => FORMAT_RGB_U8,
        (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt) => FORMAT_RGBA_U8,
        (ChannelCount::Gray, ChannelSize::_32bit, ChannelType::Float) => FORMAT_GRAY_F32,
        (ChannelCount::GrayAlpha, ChannelSize::_32bit, ChannelType::Float) => FORMAT_GRAY_ALPHA_F32,
        (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Float) => FORMAT_RGB_F32,
        (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float) => FORMAT_RGBA_F32,
        (ChannelCount::Gray, ChannelSize::_16bit, ChannelType::UInt) => FORMAT_GRAY_U16,
        (ChannelCount::GrayAlpha, ChannelSize::_16bit, ChannelType::UInt) => FORMAT_GRAY_ALPHA_U16,
        (ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::UInt) => FORMAT_RGB_U16,
        (ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt) => FORMAT_RGBA_U16,
        _ => panic!("Unsupported format for Transform: {}", format),
    }
}

/// Applies transformation to the input image using GPU.
///
/// The output image dimensions determine the size of the result.
/// Areas outside the transformed input will be transparent (RGBA 0,0,0,0) or black (RGB 0,0,0).
pub(super) fn apply(
    params: &Transform,
    ctx: &Gpu,
    pipeline: &GpuTransformPipeline,
    input: &GpuImage,
    output: &mut GpuImage,
) {
    let device = ctx.device();
    let queue = ctx.queue();
    let input_desc = *input.desc();
    let output_desc = *output.desc();

    assert_eq!(
        output_desc.color_format, input_desc.color_format,
        "Input and output must have same color format"
    );

    let format_type = get_format_type(input_desc.color_format);

    // For formats that use OR-based writing (non-word-aligned),
    // we need to clear the output buffer first to avoid garbage data
    let needs_clear = matches!(
        format_type,
        FORMAT_GRAY_U8
            | FORMAT_GRAY_ALPHA_U8
            | FORMAT_RGB_U8
            | FORMAT_GRAY_U16
            | FORMAT_GRAY_ALPHA_U16
            | FORMAT_RGB_U16
    );

    if needs_clear {
        queue.write_buffer(
            output.write_buffer().buffer(),
            0,
            &vec![0u8; output_desc.size_in_bytes()],
        );
    }

    // Compute inverse transform for backward mapping
    let inv = params.transform.inverse();

    let gpu_params = Params {
        inv_matrix: [
            inv.matrix2.x_axis.x,
            inv.matrix2.x_axis.y,
            inv.matrix2.y_axis.x,
            inv.matrix2.y_axis.y,
        ],
        inv_translation: [inv.translation.x, inv.translation.y],
        input_width: input_desc.width,
        input_height: input_desc.height,
        output_width: output_desc.width,
        output_height: output_desc.height,
        input_stride: input_desc.stride as u32,
        output_stride: output_desc.stride as u32,
        filter_mode: match params.filter {
            FilterMode::Nearest => 0,
            FilterMode::Bilinear => 1,
        },
        format_type,
    };

    // Create params buffer
    let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("transform_params_buffer"),
        contents: bytemuck::bytes_of(&gpu_params),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    // Create bind group using input/output GpuImage buffers directly
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("transform_bind_group"),
        layout: &pipeline.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: input.read_buffer().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: output.write_buffer().as_entire_binding(),
            },
        ],
    });

    // Encode and submit
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("transform_encoder"),
    });

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("transform_pass"),
            timestamp_writes: None,
        });

        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);

        // Dispatch workgroups (16x16 threads per group)
        let workgroups_x = output_desc.width.div_ceil(16);
        let workgroups_y = output_desc.height.div_ceil(16);
        pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
    }

    queue.submit(std::iter::once(encoder.finish()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_utils::{load_lena_rgba_u8, load_lena_small_rgba_u8_61x38, test_gpu};
    use std::f32::consts::PI;

    #[test]
    fn test_identity_transform() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let input_cpu = load_lena_small_rgba_u8_61x38();
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input_cpu.desc());

        let transform = Transform::new();
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        // Download and verify
        let output_cpu = output.to_image(&ctx).unwrap();

        // With identity transform, output should match input
        let input_packed = input_cpu.packed();
        let output_packed = output_cpu.packed();
        assert_eq!(input_packed.bytes(), output_packed.bytes());
    }

    #[test]
    fn test_scale_transform() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let input_cpu = load_lena_small_rgba_u8_61x38();
        let input = GpuImage::from_image(&ctx, &input_cpu);

        // Create larger output for 2x scale
        let output_format =
            ColorFormat::from((ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt));
        let output_desc = ImageDesc::new(122, 76, output_format);
        let mut output = GpuImage::new_empty(&ctx, output_desc);

        let transform = Transform::new().scale(Vec2::new(2.0, 2.0));
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        // Download and verify
        let output_cpu = output.to_image(&ctx).unwrap();

        // Check that output is not all zeros
        let non_zero = output_cpu.bytes().iter().any(|&b| b != 0);
        assert!(non_zero, "Output should contain non-zero values");
    }

    #[test]
    fn test_rotation_transform() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let input_cpu = load_lena_small_rgba_u8_61x38();
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input_cpu.desc());

        // Rotate 90 degrees around center
        let center = Vec2::new(30.5, 19.0);
        let transform = Transform::new().rotate_around(PI / 2.0, center);
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        // Download and verify
        let output_cpu = output.to_image(&ctx).unwrap();

        // Check that output is not all zeros
        let non_zero = output_cpu.bytes().iter().any(|&b| b != 0);
        assert!(non_zero, "Output should contain non-zero values");
    }

    #[test]
    fn test_filter_modes() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let input_cpu = load_lena_small_rgba_u8_61x38();
        let input = GpuImage::from_image(&ctx, &input_cpu);

        // Scale up with nearest neighbor
        let output_format =
            ColorFormat::from((ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt));
        let output_desc = ImageDesc::new(122, 76, output_format);

        let mut output_nearest = GpuImage::new_empty(&ctx, output_desc);
        let transform_nearest = Transform::new()
            .scale(Vec2::new(2.0, 2.0))
            .filter(FilterMode::Nearest);
        transform_nearest.apply_gpu(&ctx, &pipeline, &input, &mut output_nearest);

        let mut output_bilinear = GpuImage::new_empty(&ctx, output_desc);
        let transform_bilinear = Transform::new()
            .scale(Vec2::new(2.0, 2.0))
            .filter(FilterMode::Bilinear);
        transform_bilinear.apply_gpu(&ctx, &pipeline, &input, &mut output_bilinear);

        // Download and verify
        let nearest_cpu = output_nearest.to_image(&ctx).unwrap();
        let bilinear_cpu = output_bilinear.to_image(&ctx).unwrap();

        // Results should be different due to different filtering
        // (though this isn't guaranteed for all inputs)
        let nearest_packed = nearest_cpu.packed();
        let bilinear_packed = bilinear_cpu.packed();

        // At minimum, both should have produced valid output
        assert!(nearest_packed.bytes().iter().any(|&b| b != 0));
        assert!(bilinear_packed.bytes().iter().any(|&b| b != 0));
    }

    #[test]
    fn test_transform_rgb_u8() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        // Load lena and convert to RGB_U8
        let lena_rgba = load_lena_rgba_u8();
        let lena_rgb = lena_rgba.convert(ColorFormat::RGB_U8).unwrap();

        let input = GpuImage::from_image(&ctx, &lena_rgb);
        let mut output = GpuImage::new_empty(&ctx, *lena_rgb.desc());

        // Apply identity transform
        let transform = Transform::new();
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Identity transform should preserve the image
        assert_eq!(lena_rgb.bytes(), output_cpu.bytes());
    }

    #[test]
    fn test_transform_rgb_u8_rotation() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        // Load lena and convert to RGB_U8
        let lena_rgba = load_lena_rgba_u8();
        let lena_rgb = lena_rgba.convert(ColorFormat::RGB_U8).unwrap();

        let width = lena_rgb.desc().width;
        let height = lena_rgb.desc().height;
        let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);

        let input = GpuImage::from_image(&ctx, &lena_rgb);
        let mut output = GpuImage::new_empty(&ctx, *lena_rgb.desc());

        // Apply rotation
        let transform = Transform::new().rotate_around(PI / 4.0, center);
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Output should have non-zero values (rotated image)
        assert!(output_cpu.bytes().iter().any(|&b| b != 0));

        // Output should be different from input (rotated)
        assert_ne!(lena_rgb.bytes(), output_cpu.bytes());
    }

    #[test]
    fn test_transform_rgba_f32() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        // Load lena and convert to RGBA_F32
        let lena_rgba = load_lena_rgba_u8();
        let lena_f32 = lena_rgba.convert(ColorFormat::RGBA_F32).unwrap();

        let input = GpuImage::from_image(&ctx, &lena_f32);
        let mut output = GpuImage::new_empty(&ctx, *lena_f32.desc());

        // Apply identity transform
        let transform = Transform::new();
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Identity transform should preserve the image
        // Compare as f32 slices for floating point comparison
        let input_floats: &[f32] = bytemuck::cast_slice(lena_f32.bytes());
        let output_floats: &[f32] = bytemuck::cast_slice(output_cpu.bytes());

        for (i, (a, b)) in input_floats.iter().zip(output_floats.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "Mismatch at index {}: {} vs {}",
                i,
                a,
                b
            );
        }
    }

    #[test]
    fn test_transform_rgba_f32_rotation() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        // Load lena and convert to RGBA_F32
        let lena_rgba = load_lena_rgba_u8();
        let lena_f32 = lena_rgba.convert(ColorFormat::RGBA_F32).unwrap();

        let width = lena_f32.desc().width;
        let height = lena_f32.desc().height;
        let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);

        let input = GpuImage::from_image(&ctx, &lena_f32);
        let mut output = GpuImage::new_empty(&ctx, *lena_f32.desc());

        // Apply rotation
        let transform = Transform::new().rotate_around(PI / 4.0, center);
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Output should have non-zero values
        let output_floats: &[f32] = bytemuck::cast_slice(output_cpu.bytes());
        assert!(output_floats.iter().any(|&f| f != 0.0));
    }

    #[test]
    fn test_transform_rgb_f32() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        // Load lena and convert to RGB_F32
        let lena_rgba = load_lena_rgba_u8();
        let lena_f32 = lena_rgba.convert(ColorFormat::RGB_F32).unwrap();

        let input = GpuImage::from_image(&ctx, &lena_f32);
        let mut output = GpuImage::new_empty(&ctx, *lena_f32.desc());

        // Apply identity transform
        let transform = Transform::new();
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Identity transform should preserve the image
        let input_floats: &[f32] = bytemuck::cast_slice(lena_f32.bytes());
        let output_floats: &[f32] = bytemuck::cast_slice(output_cpu.bytes());

        for (i, (a, b)) in input_floats.iter().zip(output_floats.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "Mismatch at index {}: {} vs {}",
                i,
                a,
                b
            );
        }
    }

    #[test]
    fn test_transform_rgb_f32_rotation() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        // Load lena and convert to RGB_F32
        let lena_rgba = load_lena_rgba_u8();
        let lena_f32 = lena_rgba.convert(ColorFormat::RGB_F32).unwrap();

        let width = lena_f32.desc().width;
        let height = lena_f32.desc().height;
        let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);

        let input = GpuImage::from_image(&ctx, &lena_f32);
        let mut output = GpuImage::new_empty(&ctx, *lena_f32.desc());

        // Apply rotation
        let transform = Transform::new().rotate_around(PI / 4.0, center);
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Output should have non-zero values
        let output_floats: &[f32] = bytemuck::cast_slice(output_cpu.bytes());
        assert!(output_floats.iter().any(|&f| f != 0.0));
    }

    #[test]
    fn test_transform_all_formats_scale() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let lena_rgba = load_lena_rgba_u8();

        let formats = [
            ColorFormat::GRAY_U8,
            ColorFormat::GRAY_ALPHA_U8,
            ColorFormat::RGB_U8,
            ColorFormat::RGBA_U8,
            ColorFormat::GRAY_U16,
            ColorFormat::GRAY_ALPHA_U16,
            ColorFormat::RGB_U16,
            ColorFormat::RGBA_U16,
            ColorFormat::GRAY_F32,
            ColorFormat::GRAY_ALPHA_F32,
            ColorFormat::RGB_F32,
            ColorFormat::RGBA_F32,
        ];

        for format in formats {
            let input_cpu = lena_rgba.clone().convert(format).unwrap();
            let width = input_cpu.desc().width;
            let height = input_cpu.desc().height;

            // Create output with same dimensions
            let output_desc = ImageDesc::new(width / 2, height / 2, format);

            let input = GpuImage::from_image(&ctx, &input_cpu);
            let mut output = GpuImage::new_empty(&ctx, output_desc);

            // Apply scale down
            let transform = Transform::new().scale(Vec2::new(0.5, 0.5));
            transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

            let output_cpu = output.to_image(&ctx).unwrap();

            // Output should have non-zero values
            let has_non_zero = output_cpu.bytes().iter().any(|&b| b != 0);
            assert!(
                has_non_zero,
                "Output should contain non-zero values for format {:?}",
                format
            );
        }
    }

    #[test]
    fn test_transform_gray_u8_identity() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let lena_rgba = load_lena_rgba_u8();
        let lena_gray = lena_rgba.convert(ColorFormat::GRAY_U8).unwrap();

        let input = GpuImage::from_image(&ctx, &lena_gray);
        let mut output = GpuImage::new_empty(&ctx, *lena_gray.desc());

        let transform = Transform::new();
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Identity transform should preserve the image
        assert_eq!(lena_gray.bytes(), output_cpu.bytes());
    }

    #[test]
    fn test_transform_gray_alpha_u8_identity() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let lena_rgba = load_lena_rgba_u8();
        let lena_gray_alpha = lena_rgba.convert(ColorFormat::GRAY_ALPHA_U8).unwrap();

        let input = GpuImage::from_image(&ctx, &lena_gray_alpha);
        let mut output = GpuImage::new_empty(&ctx, *lena_gray_alpha.desc());

        let transform = Transform::new();
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Identity transform should preserve the image
        assert_eq!(lena_gray_alpha.bytes(), output_cpu.bytes());
    }

    #[test]
    fn test_transform_gray_f32_identity() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let lena_rgba = load_lena_rgba_u8();
        let lena_gray_f32 = lena_rgba.convert(ColorFormat::GRAY_F32).unwrap();

        let input = GpuImage::from_image(&ctx, &lena_gray_f32);
        let mut output = GpuImage::new_empty(&ctx, *lena_gray_f32.desc());

        let transform = Transform::new();
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Compare as f32 slices
        let input_floats: &[f32] = bytemuck::cast_slice(lena_gray_f32.bytes());
        let output_floats: &[f32] = bytemuck::cast_slice(output_cpu.bytes());

        for (i, (a, b)) in input_floats.iter().zip(output_floats.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "Mismatch at index {}: {} vs {}",
                i,
                a,
                b
            );
        }
    }

    #[test]
    fn test_transform_gray_alpha_f32_identity() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let lena_rgba = load_lena_rgba_u8();
        let lena_gray_alpha_f32 = lena_rgba.convert(ColorFormat::GRAY_ALPHA_F32).unwrap();

        let input = GpuImage::from_image(&ctx, &lena_gray_alpha_f32);
        let mut output = GpuImage::new_empty(&ctx, *lena_gray_alpha_f32.desc());

        let transform = Transform::new();
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Compare as f32 slices
        let input_floats: &[f32] = bytemuck::cast_slice(lena_gray_alpha_f32.bytes());
        let output_floats: &[f32] = bytemuck::cast_slice(output_cpu.bytes());

        for (i, (a, b)) in input_floats.iter().zip(output_floats.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "Mismatch at index {}: {} vs {}",
                i,
                a,
                b
            );
        }
    }

    #[test]
    fn test_transform_gray_u8_rotation() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuTransformPipeline::new(&ctx).unwrap();

        let lena_rgba = load_lena_rgba_u8();
        let lena_gray = lena_rgba.convert(ColorFormat::GRAY_U8).unwrap();

        let width = lena_gray.desc().width;
        let height = lena_gray.desc().height;
        let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);

        let input = GpuImage::from_image(&ctx, &lena_gray);
        let mut output = GpuImage::new_empty(&ctx, *lena_gray.desc());

        let transform = Transform::new().rotate_around(PI / 4.0, center);
        transform.apply_gpu(&ctx, &pipeline, &input, &mut output);

        let output_cpu = output.to_image(&ctx).unwrap();

        // Output should have non-zero values
        assert!(output_cpu.bytes().iter().any(|&b| b != 0));

        // Output should be different from input
        assert_ne!(lena_gray.bytes(), output_cpu.bytes());
    }
}
