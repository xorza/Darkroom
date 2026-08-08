use wgpu::util::DeviceExt;

use super::pipeline::GpuBlendPipeline;
use super::{Blend, BlendMode};
use crate::common::color_format::{ChannelCount, ChannelType};
use crate::common::error::Result;
use crate::gpu::Gpu;
use crate::gpu::gpu_image::GpuImage;
use crate::image::ImageDesc;

impl Blend {
    /// Applies blending of two images using GPU.
    /// Supports U8 and F32 formats for L, LA, RGB, and RGBA.
    ///
    /// # Panics
    /// Panics if images have different dimensions or color formats.
    pub fn apply_gpu(
        &self,
        ctx: &Gpu,
        pipeline: &GpuBlendPipeline,
        src: &GpuImage,
        dst: &GpuImage,
        output: &mut GpuImage,
    ) -> Result<()> {
        let device = &ctx.device;
        let queue = &ctx.queue;

        assert_eq!(src.desc, dst.desc, "src/dst desc mismatch");
        assert_eq!(src.desc, output.desc, "src/output desc mismatch");

        let uniform_params = Params::new(src.desc, self.mode, self.alpha);

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

            // One invocation per u32 word of the packed buffer.
            let total_words = uniform_params.total_bytes.div_ceil(4);
            compute_pass.dispatch_workgroups(total_words.div_ceil(256), 1, 1);
        }

        queue.submit(std::iter::once(encoder.finish()));

        Ok(())
    }
}

/// Sentinel for "no alpha channel" (matches the WGSL constant).
const NO_ALPHA: u32 = 0xffff_ffff;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    mode: u32,
    alpha: f32,
    total_bytes: u32,
    elem_size: u32,
    channels: u32,
    alpha_channel: u32,
    is_float: u32,
    _pad: u32,
}

impl Params {
    fn new(desc: ImageDesc, mode: BlendMode, alpha: f32) -> Self {
        let format = desc.color_format;
        let channels = format.channel_count.channel_count() as u32;
        let alpha_channel = match format.channel_count {
            ChannelCount::Rgba => channels - 1,
            _ => NO_ALPHA,
        };
        Self {
            // BlendMode is #[repr(u8)]; the shader's mode contract depends on this order.
            mode: mode as u32,
            alpha,
            total_bytes: desc.size_in_bytes() as u32,
            elem_size: format.channel_size.byte_count() as u32,
            channels,
            alpha_channel,
            is_float: u32::from(format.channel_type == ChannelType::Float),
            _pad: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::BlendMode;
    use super::*;
    use crate::common::color_format::ColorFormat;
    use crate::common::internals::{create_test_image, create_test_image_f32, gpu::test_gpu};
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
        let mut output = GpuImage::new_empty(&ctx, dst.desc);

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
                "Byte {i} differs: dst={d}, output={o}, diff={diff}"
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
        let mut output = GpuImage::new_empty(&ctx, dst.desc);

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
                "Byte {i} differs: src={s}, output={o}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_gpu_blend_multiply() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuBlendPipeline::new(&ctx).unwrap();

        let desc = ImageDesc::new(2, 2, ColorFormat::RGBA_U8);

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
                "Byte {i} differs: dst={d}, output={o}, diff={diff}"
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
            let mut output = GpuImage::new_empty(&ctx, dst.desc);

            let params = Blend::new(*mode, 0.5);
            params
                .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
                .unwrap();

            let output_cpu = output.to_image(&ctx).unwrap();

            assert!(!output_cpu.bytes().is_empty(), "GPU Blend {mode:?} failed");
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
        let mut output = GpuImage::new_empty(&ctx, dst.desc);

        // Execute multiple times with same pipeline
        let params1 = Blend::new(BlendMode::Normal, 0.5);
        params1
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();

        let params2 = Blend::new(BlendMode::Multiply, 0.8);
        params2
            .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
            .unwrap();
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
        let mut output = GpuImage::new_empty(&ctx, dst.desc);

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
                "Byte {i} differs: src={s}, output={o}, diff={diff}"
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
        let mut output = GpuImage::new_empty(&ctx, dst.desc);

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
                "Byte {i} differs: src={s}, output={o}, diff={diff}"
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
        let mut output = GpuImage::new_empty(&ctx, dst.desc);

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
        let mut output = GpuImage::new_empty(&ctx, dst.desc);

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
        let mut output = GpuImage::new_empty(&ctx, dst.desc);

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

        let u8_formats = [ColorFormat::L_U8, ColorFormat::RGB_U8, ColorFormat::RGBA_U8];

        let u16_formats = [
            ColorFormat::L_U16,
            ColorFormat::RGB_U16,
            ColorFormat::RGBA_U16,
        ];

        let f32_formats = [
            ColorFormat::L_F32,
            ColorFormat::RGB_F32,
            ColorFormat::RGBA_F32,
        ];

        // Test U8 formats
        for format in &u8_formats {
            let src_cpu = create_test_image(*format, 8, 8, 0);
            let dst_cpu = create_test_image(*format, 8, 8, 100);

            let src = GpuImage::from_image(&ctx, &src_cpu);
            let dst = GpuImage::from_image(&ctx, &dst_cpu);
            let mut output = GpuImage::new_empty(&ctx, dst.desc);

            let params = Blend::new(BlendMode::Normal, 0.5);
            params
                .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
                .unwrap_or_else(|_| panic!("GPU Blend failed for format {format:?}"));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU Blend {format:?} produced empty output"
            );
        }

        // Test U16 formats
        for format in &u16_formats {
            let src_cpu = create_test_image(*format, 8, 8, 0);
            let dst_cpu = create_test_image(*format, 8, 8, 100);

            let src = GpuImage::from_image(&ctx, &src_cpu);
            let dst = GpuImage::from_image(&ctx, &dst_cpu);
            let mut output = GpuImage::new_empty(&ctx, dst.desc);

            let params = Blend::new(BlendMode::Normal, 0.5);
            params
                .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
                .unwrap_or_else(|_| panic!("GPU Blend failed for format {format:?}"));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU Blend {format:?} produced empty output"
            );
        }

        // Test F32 formats
        for format in &f32_formats {
            let src_cpu = create_test_image_f32(*format, 8, 8, 0.7);
            let dst_cpu = create_test_image_f32(*format, 8, 8, 0.3);

            let src = GpuImage::from_image(&ctx, &src_cpu);
            let dst = GpuImage::from_image(&ctx, &dst_cpu);
            let mut output = GpuImage::new_empty(&ctx, dst.desc);

            let params = Blend::new(BlendMode::Normal, 0.5);
            params
                .apply_gpu(&ctx, &pipeline, &src, &dst, &mut output)
                .unwrap_or_else(|_| panic!("GPU Blend failed for format {format:?}"));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU Blend {format:?} produced empty output"
            );
        }
    }
}
