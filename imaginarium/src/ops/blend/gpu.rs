use wgpu::util::DeviceExt;

use super::pipeline::GpuBlendPipeline;
use super::{Blend, BlendMode};
use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};
use crate::common::error::{Error, Result};
use crate::gpu::Gpu;
use crate::gpu::GpuImage;

// Format type constants matching shader
const FORMAT_L_U8: u32 = 0;
const FORMAT_LA_U8: u32 = 1;
const FORMAT_RGB_U8: u32 = 2;
const FORMAT_RGBA_U8: u32 = 3;
const FORMAT_L_F32: u32 = 4;
const FORMAT_LA_F32: u32 = 5;
const FORMAT_RGB_F32: u32 = 6;
const FORMAT_RGBA_F32: u32 = 7;
const FORMAT_L_U16: u32 = 8;
const FORMAT_LA_U16: u32 = 9;
const FORMAT_RGB_U16: u32 = 10;
const FORMAT_RGBA_U16: u32 = 11;

fn get_format_type(format: ColorFormat) -> Result<u32> {
    match (
        format.channel_count,
        format.channel_size,
        format.channel_type,
    ) {
        (ChannelCount::L, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_L_U8),
        (ChannelCount::LA, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_LA_U8),
        (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_RGB_U8),
        (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_RGBA_U8),
        (ChannelCount::L, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_L_F32),
        (ChannelCount::LA, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_LA_F32),
        (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_RGB_F32),
        (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_RGBA_F32),
        (ChannelCount::L, ChannelSize::_16bit, ChannelType::UInt) => Ok(FORMAT_L_U16),
        (ChannelCount::LA, ChannelSize::_16bit, ChannelType::UInt) => Ok(FORMAT_LA_U16),
        (ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::UInt) => Ok(FORMAT_RGB_U16),
        (ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt) => Ok(FORMAT_RGBA_U16),
        _ => Err(Error::UnsupportedFormat(format!(
            "GPU blend does not support format: {}",
            format
        ))),
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    mode: u32,
    alpha: f32,
    width: u32,
    height: u32,
    stride: u32,
    format_type: u32,
    _padding: [u32; 2],
}

/// Applies blending of two images using GPU.
///
/// # Panics
/// Panics if images have different dimensions or color formats.
pub(super) fn apply(
    params: &Blend,
    ctx: &Gpu,
    pipeline: &GpuBlendPipeline,
    src: &GpuImage,
    dst: &GpuImage,
    output: &mut GpuImage,
) -> Result<()> {
    let device = ctx.device();
    let queue = ctx.queue();

    assert_eq!(src.desc(), dst.desc(), "src/dst desc mismatch");
    assert_eq!(src.desc(), output.desc(), "src/output desc mismatch");

    let format = src.desc().color_format;
    let format_type = get_format_type(format)?;

    let width = src.desc().width;
    let height = src.desc().height;
    let stride = src.desc().stride;

    let mode_u32 = match params.mode {
        BlendMode::Normal => 0u32,
        BlendMode::Add => 1u32,
        BlendMode::Subtract => 2u32,
        BlendMode::Multiply => 3u32,
        BlendMode::Screen => 4u32,
        BlendMode::Overlay => 5u32,
    };

    let uniform_params = Params {
        mode: mode_u32,
        alpha: params.alpha,
        width: width as u32,
        height: height as u32,
        stride: stride as u32,
        format_type,
        _padding: [0; 2],
    };

    let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("blend_params_buffer"),
        contents: bytemuck::cast_slice(&[uniform_params]),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blend_bind_group"),
        layout: &pipeline.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: src.read_buffer().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: dst.read_buffer().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: output.write_buffer().as_entire_binding(),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("blend_encoder"),
    });

    {
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("blend_pass"),
            timestamp_writes: None,
        });
        compute_pass.set_pipeline(&pipeline.compute_pipeline);
        compute_pass.set_bind_group(0, &bind_group, &[]);

        // Calculate work items based on format
        let work_items = match format_type {
            FORMAT_L_U8 => {
                // Each thread processes 4 pixels (one u32)
                let quads_per_row = width.div_ceil(4);
                quads_per_row * height
            }
            FORMAT_LA_U8 | FORMAT_L_U16 => {
                // Each thread processes 2 pixels (one u32)
                let pairs_per_row = width.div_ceil(2);
                pairs_per_row * height
            }
            _ => {
                // Each thread processes 1 pixel
                width * height
            }
        };
        let workgroup_count = work_items.div_ceil(256) as u32;
        compute_pass.dispatch_workgroups(workgroup_count, 1, 1);
    }

    queue.submit(std::iter::once(encoder.finish()));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_utils::{create_test_image, create_test_image_f32, test_gpu};
    use crate::image::{Image, ImageDesc};

    #[test]
    fn test_gpu_blend_normal_alpha_zero() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let src_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 0);
        let dst_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 100);

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, *dst.desc());

        let params = Blend::new(BlendMode::Normal, 0.0);
        params
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Alpha = 0 should return dst
        for (i, (d, o)) in dst_cpu
            .bytes()
            .iter()
            .zip(output_cpu.bytes().iter())
            .enumerate()
        {
            let diff = (*d as i32 - *o as i32).abs();
            assert!(
                diff <= 1,
                "Byte {} differs: dst={}, output={}, diff={}",
                i,
                d,
                o,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_blend_normal_alpha_one() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let src_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 0);
        let dst_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 100);

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, *dst.desc());

        let params = Blend::new(BlendMode::Normal, 1.0);
        params
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Alpha = 1 with Normal should return src
        for (i, (s, o)) in src_cpu
            .bytes()
            .iter()
            .zip(output_cpu.bytes().iter())
            .enumerate()
        {
            let diff = (*s as i32 - *o as i32).abs();
            assert!(
                diff <= 1,
                "Byte {} differs: src={}, output={}, diff={}",
                i,
                s,
                o,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_blend_multiply() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let desc = ImageDesc::new_with_stride(2, 2, ColorFormat::RGBA_U8);

        // White source RGB, same alpha as dst
        let mut src_data = vec![255u8; 16];
        let mut dst_data = vec![128u8; 16];
        // Set matching alpha values
        for i in (3..16).step_by(4) {
            src_data[i] = 128; // Same alpha as dst
            dst_data[i] = 128;
        }
        let src_cpu = Image::new_with_data(desc, src_data).unwrap();
        let dst_cpu = Image::new_with_data(desc, dst_data).unwrap();

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, desc);

        let params = Blend::new(BlendMode::Multiply, 1.0);
        params
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Multiply with white (1.0) RGB should return dst RGB, alpha is blended normally
        for (i, (d, o)) in dst_cpu
            .bytes()
            .iter()
            .zip(output_cpu.bytes().iter())
            .enumerate()
        {
            let diff = (*d as i32 - *o as i32).abs();
            assert!(
                diff <= 1,
                "Byte {} differs: dst={}, output={}, diff={}",
                i,
                d,
                o,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_blend_all_modes() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let modes = [
            BlendMode::Normal,
            BlendMode::Add,
            BlendMode::Subtract,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
        ];

        for mode in &modes {
            let src_cpu = create_test_image(ColorFormat::RGBA_U8, 8, 4, 0);
            let dst_cpu = create_test_image(ColorFormat::RGBA_U8, 8, 4, 100);

            let src = GpuImage::from_image(&ctx, &src_cpu);
            let dst = GpuImage::from_image(&ctx, &dst_cpu);
            let mut output = GpuImage::new_empty(&ctx, *dst.desc());

            let params = Blend::new(*mode, 0.5);
            params
                .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
                .unwrap();

            let output_cpu = output.to_image(&ctx).unwrap();

            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU Blend {:?} failed",
                mode
            );
        }
    }

    #[test]
    fn test_gpu_blend_pipeline_reuse() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let src_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 0);
        let dst_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 100);

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, *dst.desc());

        // Execute multiple times with same pipeline
        let params1 = Blend::new(BlendMode::Normal, 0.5);
        apply(&params1, &ctx, &pipeline, &src, &dst, &mut output).unwrap();

        let params2 = Blend::new(BlendMode::Multiply, 0.8);
        apply(&params2, &ctx, &pipeline, &src, &dst, &mut output).unwrap();
    }

    #[test]
    fn test_gpu_blend_gray_u8() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let src_cpu = create_test_image(ColorFormat::L_U8, 8, 8, 0);
        let dst_cpu = create_test_image(ColorFormat::L_U8, 8, 8, 100);

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, *dst.desc());

        let params = Blend::new(BlendMode::Normal, 1.0);
        params
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Alpha = 1 with Normal should return src
        for (i, (s, o)) in src_cpu
            .bytes()
            .iter()
            .zip(output_cpu.bytes().iter())
            .enumerate()
        {
            let diff = (*s as i32 - *o as i32).abs();
            assert!(
                diff <= 1,
                "Byte {} differs: src={}, output={}, diff={}",
                i,
                s,
                o,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_blend_rgb_u8() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let src_cpu = create_test_image(ColorFormat::RGB_U8, 8, 8, 0);
        let dst_cpu = create_test_image(ColorFormat::RGB_U8, 8, 8, 100);

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, *dst.desc());

        let params = Blend::new(BlendMode::Normal, 1.0);
        params
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Alpha = 1 with Normal should return src
        for (i, (s, o)) in src_cpu
            .bytes()
            .iter()
            .zip(output_cpu.bytes().iter())
            .enumerate()
        {
            let diff = (*s as i32 - *o as i32).abs();
            assert!(
                diff <= 1,
                "Byte {} differs: src={}, output={}, diff={}",
                i,
                s,
                o,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_blend_rgba_f32() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let src_cpu = create_test_image_f32(ColorFormat::RGBA_F32, 4, 4, 0.8);
        let dst_cpu = create_test_image_f32(ColorFormat::RGBA_F32, 4, 4, 0.3);

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, *dst.desc());

        let params = Blend::new(BlendMode::Normal, 1.0);
        params
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Check that output has src values (alpha=1, normal mode)
        let src_bytes = src_cpu.bytes();
        let out_bytes = output_cpu.bytes();
        for i in (0..src_bytes.len()).step_by(4) {
            let src_val = f32::from_le_bytes([
                src_bytes[i],
                src_bytes[i + 1],
                src_bytes[i + 2],
                src_bytes[i + 3],
            ]);
            let out_val = f32::from_le_bytes([
                out_bytes[i],
                out_bytes[i + 1],
                out_bytes[i + 2],
                out_bytes[i + 3],
            ]);
            let diff = (src_val - out_val).abs();
            assert!(
                diff < 0.001,
                "Float {} differs: src={}, output={}, diff={}",
                i / 4,
                src_val,
                out_val,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_blend_rgb_f32() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let src_cpu = create_test_image_f32(ColorFormat::RGB_F32, 4, 4, 0.7);
        let dst_cpu = create_test_image_f32(ColorFormat::RGB_F32, 4, 4, 0.2);

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, *dst.desc());

        let params = Blend::new(BlendMode::Normal, 1.0);
        params
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Just verify it runs without error and produces output
        assert!(!output_cpu.bytes().is_empty());
    }

    #[test]
    fn test_gpu_blend_gray_f32() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let src_cpu = create_test_image_f32(ColorFormat::L_F32, 8, 8, 0.9);
        let dst_cpu = create_test_image_f32(ColorFormat::L_F32, 8, 8, 0.1);

        let src = GpuImage::from_image(&ctx, &src_cpu);
        let dst = GpuImage::from_image(&ctx, &dst_cpu);
        let mut output = GpuImage::new_empty(&ctx, *dst.desc());

        let params = Blend::new(BlendMode::Normal, 1.0);
        params
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Check that output has src values (alpha=1, normal mode)
        let src_bytes = src_cpu.bytes();
        let out_bytes = output_cpu.bytes();
        for i in (0..src_bytes.len()).step_by(4) {
            let src_val = f32::from_le_bytes([
                src_bytes[i],
                src_bytes[i + 1],
                src_bytes[i + 2],
                src_bytes[i + 3],
            ]);
            let out_val = f32::from_le_bytes([
                out_bytes[i],
                out_bytes[i + 1],
                out_bytes[i + 2],
                out_bytes[i + 3],
            ]);
            let diff = (src_val - out_val).abs();
            assert!(
                diff < 0.001,
                "Float {} differs: src={}, output={}, diff={}",
                i / 4,
                src_val,
                out_val,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_blend_all_formats() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let u8_formats = [
            ColorFormat::L_U8,
            ColorFormat::LA_U8,
            ColorFormat::RGB_U8,
            ColorFormat::RGBA_U8,
        ];

        let u16_formats = [
            ColorFormat::L_U16,
            ColorFormat::LA_U16,
            ColorFormat::RGB_U16,
            ColorFormat::RGBA_U16,
        ];

        let f32_formats = [
            ColorFormat::L_F32,
            ColorFormat::LA_F32,
            ColorFormat::RGB_F32,
            ColorFormat::RGBA_F32,
        ];

        // Test U8 formats
        for format in &u8_formats {
            let src_cpu = create_test_image(*format, 8, 8, 0);
            let dst_cpu = create_test_image(*format, 8, 8, 100);

            let src = GpuImage::from_image(&ctx, &src_cpu);
            let dst = GpuImage::from_image(&ctx, &dst_cpu);
            let mut output = GpuImage::new_empty(&ctx, *dst.desc());

            let params = Blend::new(BlendMode::Normal, 0.5);
            params
                .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
                .unwrap_or_else(|_| panic!("GPU Blend failed for format {:?}", format));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU Blend {:?} produced empty output",
                format
            );
        }

        // Test U16 formats
        for format in &u16_formats {
            let src_cpu = create_test_image(*format, 8, 8, 0);
            let dst_cpu = create_test_image(*format, 8, 8, 100);

            let src = GpuImage::from_image(&ctx, &src_cpu);
            let dst = GpuImage::from_image(&ctx, &dst_cpu);
            let mut output = GpuImage::new_empty(&ctx, *dst.desc());

            let params = Blend::new(BlendMode::Normal, 0.5);
            params
                .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
                .unwrap_or_else(|_| panic!("GPU Blend failed for format {:?}", format));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU Blend {:?} produced empty output",
                format
            );
        }

        // Test F32 formats
        for format in &f32_formats {
            let src_cpu = create_test_image_f32(*format, 8, 8, 0.7);
            let dst_cpu = create_test_image_f32(*format, 8, 8, 0.3);

            let src = GpuImage::from_image(&ctx, &src_cpu);
            let dst = GpuImage::from_image(&ctx, &dst_cpu);
            let mut output = GpuImage::new_empty(&ctx, *dst.desc());

            let params = Blend::new(BlendMode::Normal, 0.5);
            params
                .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
                .unwrap_or_else(|_| panic!("GPU Blend failed for format {:?}", format));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU Blend {:?} produced empty output",
                format
            );
        }
    }
}
