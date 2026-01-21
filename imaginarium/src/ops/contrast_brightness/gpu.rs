use wgpu::util::DeviceExt;

use super::ContrastBrightness;
use super::pipeline::GpuContrastBrightnessPipeline;
use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};
use crate::common::error::{Error, Result};
use crate::gpu::{Gpu, GpuImage};

// Format type constants matching shader
const FORMAT_GRAY_U8: u32 = 0;
const FORMAT_GRAY_ALPHA_U8: u32 = 1;
const FORMAT_RGB_U8: u32 = 2;
const FORMAT_RGBA_U8: u32 = 3;
const FORMAT_GRAY_F32: u32 = 4;
const FORMAT_GRAY_ALPHA_F32: u32 = 5;
const FORMAT_RGB_F32: u32 = 6;
const FORMAT_RGBA_F32: u32 = 7;

fn get_format_type(format: ColorFormat) -> Result<u32> {
    match (
        format.channel_count,
        format.channel_size,
        format.channel_type,
    ) {
        (ChannelCount::Gray, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_GRAY_U8),
        (ChannelCount::GrayAlpha, ChannelSize::_8bit, ChannelType::UInt) => {
            Ok(FORMAT_GRAY_ALPHA_U8)
        }
        (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_RGB_U8),
        (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_RGBA_U8),
        (ChannelCount::Gray, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_GRAY_F32),
        (ChannelCount::GrayAlpha, ChannelSize::_32bit, ChannelType::Float) => {
            Ok(FORMAT_GRAY_ALPHA_F32)
        }
        (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_RGB_F32),
        (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_RGBA_F32),
        _ => Err(Error::UnsupportedFormat(format!(
            "GPU contrast/brightness does not support format: {}",
            format
        ))),
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    contrast: f32,
    brightness: f32,
    width: u32,
    height: u32,
    stride: u32,
    format_type: u32,
    _padding: [u32; 2],
}

/// Applies contrast and brightness adjustment using GPU.
pub(super) fn apply(
    params: &ContrastBrightness,
    ctx: &Gpu,
    pipeline: &GpuContrastBrightnessPipeline,
    input: &GpuImage,
    output: &mut GpuImage,
) -> Result<()> {
    let device = ctx.device();
    let queue = ctx.queue();

    assert_eq!(input.desc(), output.desc(), "input/output desc mismatch");

    let format = input.desc().color_format;
    let format_type = get_format_type(format)?;

    let width = input.desc().width;
    let height = input.desc().height;
    let stride = input.desc().stride;

    let uniform_params = Params {
        contrast: params.contrast,
        brightness: params.brightness,
        width,
        height,
        stride: stride as u32,
        format_type,
        _padding: [0; 2],
    };

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

        // Calculate work items based on format
        let work_items = match format_type {
            FORMAT_GRAY_U8 => {
                // Each thread processes 4 pixels (one u32)
                let quads_per_row = width.div_ceil(4);
                quads_per_row * height
            }
            FORMAT_GRAY_ALPHA_U8 => {
                // Each thread processes 2 pixels (one u32)
                let pairs_per_row = width.div_ceil(2);
                pairs_per_row * height
            }
            _ => {
                // Each thread processes 1 pixel
                width * height
            }
        };
        let workgroup_count = work_items.div_ceil(256);
        compute_pass.dispatch_workgroups(workgroup_count, 1, 1);
    }

    queue.submit(std::iter::once(encoder.finish()));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{Image, ImageDesc};

    fn create_test_image(format: ColorFormat, width: u32, height: u32, seed: usize) -> Image {
        let desc = ImageDesc::new(width, height, format);
        let mut img = Image::new_empty(desc).unwrap();
        for (i, byte) in img.bytes_mut().iter_mut().enumerate() {
            *byte = ((i + seed) * 37 % 256) as u8;
        }
        img
    }

    fn create_test_image_f32(format: ColorFormat, width: u32, height: u32, value: f32) -> Image {
        let desc = ImageDesc::new(width, height, format);
        let mut img = Image::new_empty(desc).unwrap();
        let bytes = img.bytes_mut();
        // Fill with f32 values
        for chunk in bytes.chunks_exact_mut(4) {
            let val_bytes = value.to_le_bytes();
            chunk.copy_from_slice(&val_bytes);
        }
        img
    }

    #[test]
    fn test_gpu_contrast_brightness_no_change() {
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

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
                "Byte {} differs: input={}, output={}, diff={}",
                i,
                inp,
                out,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_increase() {
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::RGBA_U8, 8, 8, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

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
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let desc = ImageDesc::new(4, 4, ColorFormat::RGBA_U8);
        let mut input_data = vec![0u8; desc.stride * desc.height as usize];
        // Set specific alpha values
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.stride + x * 4;
                input_data[idx] = 100; // R
                input_data[idx + 1] = 150; // G
                input_data[idx + 2] = 200; // B
                input_data[idx + 3] = ((x + y * 4) * 16) as u8; // A - unique per pixel
            }
        }
        let input_cpu = Image::new_with_data(desc, input_data).unwrap();

        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

        let params = ContrastBrightness::new(2.0, 0.2);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Check alpha is preserved
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.stride + x * 4;
                let expected_alpha = ((x + y * 4) * 16) as u8;
                assert_eq!(
                    output_cpu.bytes()[idx + 3],
                    expected_alpha,
                    "Alpha mismatch at ({}, {})",
                    x,
                    y
                );
            }
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_pipeline_reuse() {
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::RGBA_U8, 4, 4, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

        // Execute multiple times with same pipeline
        let params1 = ContrastBrightness::new(1.5, 0.1);
        apply(&params1, &ctx, &pipeline, &input, &mut output).unwrap();

        let params2 = ContrastBrightness::new(0.5, -0.1);
        apply(&params2, &ctx, &pipeline, &input, &mut output).unwrap();
    }

    #[test]
    fn test_gpu_contrast_brightness_gray_u8() {
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::GRAY_U8, 8, 8, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

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
                "Byte {} differs: input={}, output={}, diff={}",
                i,
                inp,
                out,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_rgb_u8() {
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image(ColorFormat::RGB_U8, 8, 8, 0);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

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
                "Byte {} differs: input={}, output={}, diff={}",
                i,
                inp,
                out,
                diff
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_rgba_f32() {
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image_f32(ColorFormat::RGBA_F32, 4, 4, 0.5);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

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
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let input_cpu = create_test_image_f32(ColorFormat::GRAY_F32, 8, 8, 0.5);
        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

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
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let u8_formats = [
            ColorFormat::GRAY_U8,
            ColorFormat::GRAY_ALPHA_U8,
            ColorFormat::RGB_U8,
            ColorFormat::RGBA_U8,
        ];

        let f32_formats = [
            ColorFormat::GRAY_F32,
            ColorFormat::GRAY_ALPHA_F32,
            ColorFormat::RGB_F32,
            ColorFormat::RGBA_F32,
        ];

        // Test U8 formats
        for format in &u8_formats {
            let input_cpu = create_test_image(*format, 8, 8, 0);
            let input = GpuImage::from_image(&ctx, &input_cpu);
            let mut output = GpuImage::new_empty(&ctx, *input.desc());

            let params = ContrastBrightness::new(1.5, 0.1);
            params
                .apply_gpu(&ctx, &pipeline, &input, &mut output)
                .unwrap_or_else(|_| {
                    panic!("GPU contrast/brightness failed for format {:?}", format)
                });

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU contrast/brightness {:?} produced empty output",
                format
            );
        }

        // Test F32 formats
        for format in &f32_formats {
            let input_cpu = create_test_image_f32(*format, 8, 8, 0.5);
            let input = GpuImage::from_image(&ctx, &input_cpu);
            let mut output = GpuImage::new_empty(&ctx, *input.desc());

            let params = ContrastBrightness::new(1.5, 0.1);
            params
                .apply_gpu(&ctx, &pipeline, &input, &mut output)
                .unwrap_or_else(|_| {
                    panic!("GPU contrast/brightness failed for format {:?}", format)
                });

            let output_cpu = output.to_image(&ctx).unwrap();
            assert!(
                !output_cpu.bytes().is_empty(),
                "GPU contrast/brightness {:?} produced empty output",
                format
            );
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_gray_alpha_u8_alpha_preserved() {
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let desc = ImageDesc::new(4, 4, ColorFormat::GRAY_ALPHA_U8);
        let mut input_data = vec![0u8; desc.stride * desc.height as usize];
        // Set specific alpha values
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.stride + x * 2;
                input_data[idx] = 100; // Gray
                input_data[idx + 1] = ((x + y * 4) * 16) as u8; // A - unique per pixel
            }
        }
        let input_cpu = Image::new_with_data(desc, input_data).unwrap();

        let input = GpuImage::from_image(&ctx, &input_cpu);
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

        let params = ContrastBrightness::new(2.0, 0.2);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Check alpha is preserved
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.stride + x * 2;
                let expected_alpha = ((x + y * 4) * 16) as u8;
                assert_eq!(
                    output_cpu.bytes()[idx + 1],
                    expected_alpha,
                    "Alpha mismatch at ({}, {})",
                    x,
                    y
                );
            }
        }
    }

    #[test]
    fn test_gpu_contrast_brightness_rgba_f32_alpha_preserved() {
        let ctx = match Gpu::new() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Skipping test - no GPU available: {}", e);
                return;
            }
        };

        let pipeline = GpuContrastBrightnessPipeline::new(&ctx).unwrap();

        let desc = ImageDesc::new(4, 4, ColorFormat::RGBA_F32);
        let mut input_data = vec![0u8; desc.stride * desc.height as usize];
        // Set specific alpha values
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.stride + x * 16;
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
        let mut output = GpuImage::new_empty(&ctx, *input.desc());

        let params = ContrastBrightness::new(2.0, 0.2);
        params
            .apply_gpu(&ctx, &pipeline, &input, &mut output)
            .unwrap();

        let output_cpu = output.to_image(&ctx).unwrap();

        // Check alpha is preserved
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = y * desc.stride + x * 16;
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
                    "Alpha mismatch at ({}, {}): expected={}, got={}",
                    x,
                    y,
                    expected_alpha,
                    out_alpha
                );
            }
        }
    }
}
