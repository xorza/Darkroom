use wgpu::util::DeviceExt;

use super::ContrastBrightness;
use super::pipeline::GpuContrastBrightnessPipeline;
use crate::common::color_format::{ChannelCount, ChannelType};
use crate::common::error::Result;
use crate::gpu::Gpu;
use crate::gpu::gpu_image::GpuImage;
use crate::image::ImageDesc;
use crate::processing_context::ProcessingContext;
use crate::processing_context::image_buffer::ImageBuffer;

impl ContrastBrightness {
    /// Applies contrast and brightness adjustment using GPU.
    ///
    /// # Panics
    /// Panics if images have different dimensions or color formats.
    pub fn apply_gpu(
        &self,
        ctx: &Gpu,
        pipeline: &GpuContrastBrightnessPipeline,
        input: &GpuImage,
        output: &mut GpuImage,
    ) -> Result<()> {
        let device = &ctx.device;
        let queue = &ctx.queue;

        assert_eq!(input.desc, output.desc, "input/output desc mismatch");

        let uniform_params = Params::new(input.desc, self.contrast, self.brightness);

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("contrast_brightness_params_buffer"),
            contents: bytemuck::cast_slice(&[uniform_params]),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("contrast_brightness_bind_group"),
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

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("contrast_brightness_encoder"),
        });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("contrast_brightness_pass"),
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

    /// Applies the operation using GPU with ImageBuffer.
    ///
    /// Automatically uploads images to GPU if needed.
    pub fn execute_gpu(
        &self,
        ctx: &mut ProcessingContext,
        input: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let input_gpu = input.make_gpu(ctx)?;
        let output_gpu = output.make_gpu_mut(ctx)?;

        let gpu_processing_ctx = ctx
            .gpu_context()
            .expect("GPU context required for contrast/brightness");

        let gpu_ctx = gpu_processing_ctx.gpu.clone();
        let pipeline = gpu_processing_ctx.get_or_create(GpuContrastBrightnessPipeline::new)?;

        self.apply_gpu(&gpu_ctx, pipeline, &input_gpu, output_gpu)
    }
}

/// Sentinel for "no alpha channel to preserve" (matches the WGSL constant).
const NO_ALPHA: u32 = 0xffff_ffff;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    contrast: f32,
    brightness: f32,
    total_bytes: u32,
    elem_size: u32,
    channels: u32,
    alpha_channel: u32,
    is_float: u32,
    _pad: u32,
}

impl Params {
    fn new(desc: ImageDesc, contrast: f32, brightness: f32) -> Self {
        let format = desc.color_format;
        let channels = format.channel_count.channel_count() as u32;
        let alpha_channel = match format.channel_count {
            ChannelCount::Rgba => channels - 1,
            _ => NO_ALPHA,
        };
        Self {
            contrast,
            brightness,
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
    use super::*;
    use crate::common::color_format::ColorFormat;
    use crate::common::test_utils::{create_test_image, create_test_image_f32, test_gpu};
    use crate::image::{Image, ImageDesc};

    #[test]
    fn test_gpu_contrast_brightness_no_change() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        let params = ContrastBrightness::new(1.0, 0.0);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // No change should return input
        for (i, (inp, out)) in input_cpu
            .bytes()
            .iter()
            .zip(output_cpu.bytes().iter())
            .enumerate()
        {
            let diff = (*inp as i32 - *out as i32).abs();
            assert!(
                diff <= 1,
                "Byte {i} differs: input={inp}, output={out}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_increase() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::RGBA_U8, 8, 8, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        let params = ContrastBrightness::new(1.5, 0.1);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Output should be different from input
        let mut different = false;
        for (inp, out) in input_cpu.bytes().iter().zip(output_cpu.bytes().iter()) {
            if inp != out {
                different = true;
                break;
            }
        }
        assert!(different, "Output should be different from input");
    }

    #[test]
    fn test_gpu_contrast_brightness_alpha_preserved() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let desc = ImageDesc::new(4, 4, ColorFormat::RGBA_U8);
        let mut input_data = vec![0u8; desc.row_bytes() * desc.height];
        // Set specific alpha values
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.row_bytes() + x * 4;
                input_data[idx] = 100; // R
                input_data[idx + 1] = 150; // G
                input_data[idx + 2] = 200; // B
                input_data[idx + 3] = ((x + y * 4) * 16) as u8; // A - unique per pixel
            }
        }
        let input_cpu = Image::new_with_data(desc, input_data).unwrap();

        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        let params = ContrastBrightness::new(2.0, 0.2);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Check alpha is preserved
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.row_bytes() + x * 4;
                let expected_alpha = ((x + y * 4) * 16) as u8;
                assert_eq!(
                    output_cpu.bytes()[idx + 3],
                    expected_alpha,
                    "Alpha mismatch at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_pipeline_reuse() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        // Execute multiple times with same pipeline
        let params1 = ContrastBrightness::new(1.5, 0.1);
        params1
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let params2 = ContrastBrightness::new(0.5, -0.1);
        params2
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();
    }

    #[test]
    fn test_gpu_contrast_brightness_gray_u8() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::L_U8, 8, 8, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        let params = ContrastBrightness::new(1.0, 0.0);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // No change should return input
        for (i, (inp, out)) in input_cpu
            .bytes()
            .iter()
            .zip(output_cpu.bytes().iter())
            .enumerate()
        {
            let diff = (*inp as i32 - *out as i32).abs();
            assert!(
                diff <= 1,
                "Byte {i} differs: input={inp}, output={out}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_rgb_u8() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::RGB_U8, 8, 8, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        let params = ContrastBrightness::new(1.0, 0.0);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // No change should return input
        for (i, (inp, out)) in input_cpu
            .bytes()
            .iter()
            .zip(output_cpu.bytes().iter())
            .enumerate()
        {
            let diff = (*inp as i32 - *out as i32).abs();
            assert!(
                diff <= 1,
                "Byte {i} differs: input={inp}, output={out}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_rgba_f32() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image_f32(ColorFormat::RGBA_F32, 4, 4, 0.5);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        // No change (contrast=1, brightness=0) at mid value (0.5) should return same
        let params = ContrastBrightness::new(1.0, 0.0);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        let in_bytes = input_cpu.bytes();
        let out_bytes = output_cpu.bytes();
        for i in (0..in_bytes.len()).step_by(4) {
            let in_val = f32::from_le_bytes([
                in_bytes[i],
                in_bytes[i + 1],
                in_bytes[i + 2],
                in_bytes[i + 3],
            ]);
            let out_val = f32::from_le_bytes([
                out_bytes[i],
                out_bytes[i + 1],
                out_bytes[i + 2],
                out_bytes[i + 3],
            ]);
            let diff = (in_val - out_val).abs();
            assert!(
                diff < 0.001,
                "Float {} differs: input={}, output={}, diff={}",
                i / 4,
                in_val,
                out_val,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_gray_f32() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image_f32(ColorFormat::L_F32, 8, 8, 0.5);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        let params = ContrastBrightness::new(1.0, 0.0);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        let in_bytes = input_cpu.bytes();
        let out_bytes = output_cpu.bytes();
        for i in (0..in_bytes.len()).step_by(4) {
            let in_val = f32::from_le_bytes([
                in_bytes[i],
                in_bytes[i + 1],
                in_bytes[i + 2],
                in_bytes[i + 3],
            ]);
            let out_val = f32::from_le_bytes([
                out_bytes[i],
                out_bytes[i + 1],
                out_bytes[i + 2],
                out_bytes[i + 3],
            ]);
            let diff = (in_val - out_val).abs();
            assert!(
                diff < 0.001,
                "Float {} differs: input={}, output={}, diff={}",
                i / 4,
                in_val,
                out_val,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_all_formats() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

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
            let input_cpu = create_test_image(*format, 8, 8, 0);
            let input = GpuImage::from_image(&ctx, &input_cpu);
            let mut output = GpuImage::new_empty(&ctx, input.desc);

            let params = ContrastBrightness::new(1.5, 0.1);
            params
                .apply_gpu(&ctx, &pipeline, &input, &mut output)
                .unwrap_or_else(|_| panic!("GPU contrast/brightness failed for format {format:?}"));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU contrast/brightness {format:?} produced empty output"
            );
        }

        // Test U16 formats
        for format in &u16_formats {
            let input_cpu = create_test_image(*format, 8, 8, 0);
            let input = GpuImage::from_image(&ctx, &input_cpu);
            let mut output = GpuImage::new_empty(&ctx, input.desc);

            let params = ContrastBrightness::new(1.5, 0.1);
            params
                .apply_gpu(&ctx, &pipeline, &input, &mut output)
                .unwrap_or_else(|_| panic!("GPU contrast/brightness failed for format {format:?}"));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU contrast/brightness {format:?} produced empty output"
            );
        }

        // Test F32 formats
        for format in &f32_formats {
            let input_cpu = create_test_image_f32(*format, 8, 8, 0.5);
            let input = GpuImage::from_image(&ctx, &input_cpu);
            let mut output = GpuImage::new_empty(&ctx, input.desc);

            let params = ContrastBrightness::new(1.5, 0.1);
            params
                .apply_gpu(&ctx, &pipeline, &input, &mut output)
                .unwrap_or_else(|_| panic!("GPU contrast/brightness failed for format {format:?}"));

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU contrast/brightness {format:?} produced empty output"
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_rgba_f32_alpha_preserved() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let desc = ImageDesc::new(4, 4, ColorFormat::RGBA_F32);
        let mut input_data = vec![0u8; desc.row_bytes() * desc.height];
        // Set specific alpha values
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.row_bytes() + x * 16;
                // R, G, B = 0.5
                let rgb_bytes = 0.5f32.to_le_bytes();
                input_data[idx..idx + 4].copy_from_slice(&rgb_bytes);
                input_data[idx + 4..idx + 8].copy_from_slice(&rgb_bytes);
                input_data[idx + 8..idx + 12].copy_from_slice(&rgb_bytes);
                // A = unique per pixel
                let alpha = (x as f32 + y as f32 * 4.0) / 16.0;
                let alpha_bytes = alpha.to_le_bytes();
                input_data[idx + 12..idx + 16].copy_from_slice(&alpha_bytes);
            }
        }
        let input_cpu = Image::new_with_data(desc, input_data).unwrap();

        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, input.desc);

        let params = ContrastBrightness::new(2.0, 0.2);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Check alpha is preserved
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.row_bytes() + x * 16;
                let expected_alpha = (x as f32 + y as f32 * 4.0) / 16.0;
                let out_alpha = f32::from_le_bytes([
                    output_cpu.bytes()[idx + 12],
                    output_cpu.bytes()[idx + 13],
                    output_cpu.bytes()[idx + 14],
                    output_cpu.bytes()[idx + 15],
                ]);
                let diff = (expected_alpha - out_alpha).abs();
                assert!(
                    diff < 0.001,
                    "Alpha mismatch at ({x}, {y}): expected={expected_alpha}, got={out_alpha}"
                );
            }
        }
    }
}
