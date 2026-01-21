use std::f32::consts::PI;

use glam::Vec2;

use crate::common::test_utils::load_lena_rgba_u8;
use crate::prelude::*;
use crate::processing_context::{ImageBuffer, ProcessingContext};

#[test]
fn test_chained_operations() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        // Skip test if no GPU available
        return;
    }

    // Create test images
    let input_cpu = load_lena_rgba_u8();
    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;
    let mut input = ImageBuffer::from_cpu(input_cpu.clone());
    let mut output = ImageBuffer::new_empty(*input_cpu.desc());

    // Chain 1: Transform (rotate 45 degrees)
    let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);
    let transform = Transform::default().rotate_around(PI / 4.0, center);
    transform
        .execute_gpu(&mut ctx, &input, &mut output)
        .unwrap();

    // Swap buffers for next operation
    std::mem::swap(&mut input, &mut output);

    // Chain 2: Another transform (scale down)
    let transform = Transform::default()
        .scale(Vec2::new(0.8, 0.8))
        .translate(Vec2::new(width as f32 * 0.1, height as f32 * 0.1));
    transform
        .execute_gpu(&mut ctx, &input, &mut output)
        .unwrap();

    // Download result to verify it worked
    let result_cpu = output.make_cpu(&ctx).unwrap();

    assert_eq!(result_cpu.desc().width, width);
    assert_eq!(result_cpu.desc().height, height);
}

#[test]
fn test_mixed_gpu_cpu_operations() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        return;
    }

    // Create test images
    let input_cpu = load_lena_rgba_u8();
    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;
    let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);
    let mut buffer_a = ImageBuffer::from_cpu(input_cpu.clone());
    let mut buffer_b = ImageBuffer::new_empty(*input_cpu.desc());

    // GPU operation: Transform
    let transform = Transform::default().rotate_around(PI / 6.0, center);
    transform
        .execute_gpu(&mut ctx, &buffer_a, &mut buffer_b)
        .unwrap();

    // CPU operation: ContrastBrightness (will download from GPU)
    let contrast = ContrastBrightness::default().contrast(1.2).brightness(0.1);
    contrast
        .execute_cpu(&mut ctx, &buffer_b, &mut buffer_a)
        .unwrap();

    // After CPU operation, buffer_a should have CPU data
    assert!(buffer_a.is_cpu());

    let result = buffer_a.make_cpu(&ctx).unwrap();
    assert_eq!(result.desc().width, width);
    assert_eq!(result.desc().height, height);
}

#[test]
fn test_blend_chain() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        return;
    }

    // Create test images
    let img1 = load_lena_rgba_u8();
    let img2 = img1.clone();
    let width = img1.desc().width;
    let height = img1.desc().height;
    let output_img = Image::new_black(*img1.desc()).unwrap();

    let src = ImageBuffer::from_cpu(img1);
    let dst = ImageBuffer::from_cpu(img2);
    let mut output = ImageBuffer::from_cpu(output_img);

    // Blend two images (GPU)
    let blend = Blend::default().mode(BlendMode::Screen).alpha(0.5);
    blend
        .execute_gpu(&mut ctx, &src, &dst, &mut output)
        .unwrap();

    // Verify result is on GPU, download to check
    let result_cpu = output.make_cpu(&ctx).unwrap();
    assert_eq!(result_cpu.desc().width, width);
    assert_eq!(result_cpu.desc().height, height);
}

#[test]
fn test_multiple_transforms_ping_pong() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        return;
    }

    let input_cpu = load_lena_rgba_u8();
    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;
    let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);
    let mut buffer_a = ImageBuffer::from_cpu(input_cpu.clone());
    let mut buffer_b = ImageBuffer::new_empty(*input_cpu.desc());

    // Apply multiple transforms ping-ponging between buffers
    for i in 0..4 {
        let angle = PI / 12.0 * (i + 1) as f32;
        let transform = Transform::default().rotate_around(angle, center);

        transform
            .execute_gpu(&mut ctx, &buffer_a, &mut buffer_b)
            .unwrap();
        std::mem::swap(&mut buffer_a, &mut buffer_b);
    }

    // Final result is in buffer_a after swaps
    let result_cpu = buffer_a.make_cpu(&ctx).unwrap();

    assert_eq!(result_cpu.desc().width, width);
    assert_eq!(result_cpu.desc().height, height);
}

#[test]
fn test_full_pipeline() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        return;
    }

    // Create test images
    let input_cpu = load_lena_rgba_u8();
    let overlay_cpu = input_cpu.clone();
    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;

    let mut main_buffer = ImageBuffer::from_cpu(input_cpu.clone());
    let mut temp_buffer = ImageBuffer::new_empty(*input_cpu.desc());
    let overlay_buffer = ImageBuffer::from_cpu(overlay_cpu);

    // Step 1: Transform the main image (GPU)
    let transform = Transform::default()
        .scale(Vec2::new(0.9, 0.9))
        .translate(Vec2::new(width as f32 * 0.05, height as f32 * 0.05));
    transform
        .execute_gpu(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();

    // Step 2: Adjust contrast/brightness (CPU - will download)
    let contrast = ContrastBrightness::default().contrast(1.1).brightness(0.05);
    contrast
        .execute_cpu(&mut ctx, &temp_buffer, &mut main_buffer)
        .unwrap();

    // Step 3: Blend with overlay (GPU)
    let blend = Blend::default().mode(BlendMode::Overlay).alpha(0.3);
    blend
        .execute_gpu(&mut ctx, &overlay_buffer, &main_buffer, &mut temp_buffer)
        .unwrap();

    // Verify final result - download from GPU to check
    let result_cpu = temp_buffer.make_cpu(&ctx).unwrap();
    assert_eq!(result_cpu.desc().width, width);
    assert_eq!(result_cpu.desc().height, height);
}

// Tests using Op::apply() which auto-selects CPU or GPU

#[test]
fn test_apply_auto_selects_gpu_when_data_on_gpu() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        return;
    }

    let input_cpu = load_lena_rgba_u8();
    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;

    let input = ImageBuffer::from_cpu(input_cpu.clone());
    let mut output = ImageBuffer::new_empty(*input_cpu.desc());

    // Upload to GPU first
    input.make_gpu(&ctx).unwrap();
    assert!(input.is_gpu());

    // apply() should auto-select GPU since data is on GPU
    let contrast = ContrastBrightness::default().contrast(1.2).brightness(0.1);
    contrast.execute(&mut ctx, &input, &mut output).unwrap();

    // Output should be on GPU since input was on GPU
    assert!(output.is_gpu());

    let result = output.make_cpu(&ctx).unwrap();
    assert_eq!(result.desc().width, width);
    assert_eq!(result.desc().height, height);
}

#[test]
fn test_apply_uses_cpu_when_data_on_cpu() {
    let mut ctx = ProcessingContext::new();

    let input_cpu = load_lena_rgba_u8();
    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;

    let input = ImageBuffer::from_cpu(input_cpu.clone());
    let mut output = ImageBuffer::new_empty(*input_cpu.desc());

    // Data starts on CPU
    assert!(input.is_cpu());

    // apply() should use CPU since data is on CPU
    let contrast = ContrastBrightness::default().contrast(1.2).brightness(0.1);
    contrast.execute(&mut ctx, &input, &mut output).unwrap();

    // Output should be on CPU
    assert!(output.is_cpu());

    let result_cpu = output.make_cpu(&ctx).unwrap();
    assert_eq!(result_cpu.desc().width, width);
    assert_eq!(result_cpu.desc().height, height);
}

#[test]
fn test_apply_chained_operations() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        return;
    }

    let input_cpu = load_lena_rgba_u8();
    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;
    let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);

    let mut buffer_a = ImageBuffer::from_cpu(input_cpu.clone());
    let mut buffer_b = ImageBuffer::new_empty(*input_cpu.desc());

    // Chain multiple operations using apply()
    // Step 1: Transform (will use GPU)
    Transform::default()
        .rotate_around(PI / 6.0, center)
        .execute(&mut ctx, &buffer_a, &mut buffer_b)
        .unwrap();
    std::mem::swap(&mut buffer_a, &mut buffer_b);

    // Step 2: Contrast/brightness (will auto-select based on data location)
    ContrastBrightness::default()
        .contrast(1.3)
        .brightness(0.05)
        .execute(&mut ctx, &buffer_a, &mut buffer_b)
        .unwrap();
    std::mem::swap(&mut buffer_a, &mut buffer_b);

    // Step 3: Another transform
    Transform::default()
        .scale(Vec2::new(0.9, 0.9))
        .translate(Vec2::new(width as f32 * 0.05, height as f32 * 0.05))
        .execute(&mut ctx, &buffer_a, &mut buffer_b)
        .unwrap();

    let result = buffer_b.make_cpu(&ctx).unwrap();
    assert_eq!(result.desc().width, width);
    assert_eq!(result.desc().height, height);
}

#[test]
fn test_apply_blend_chain() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        return;
    }

    let img1 = load_lena_rgba_u8();
    let img2 = img1.clone();
    let width = img1.desc().width;
    let height = img1.desc().height;

    let src = ImageBuffer::from_cpu(img1);
    let dst = ImageBuffer::from_cpu(img2);
    let mut output = ImageBuffer::new_empty(*src.desc());

    // Blend using apply()
    Blend::default()
        .mode(BlendMode::Multiply)
        .alpha(0.7)
        .execute(&mut ctx, &src, &dst, &mut output)
        .unwrap();

    let result = output.make_cpu(&ctx).unwrap();
    assert_eq!(result.desc().width, width);
    assert_eq!(result.desc().height, height);
}

#[test]
fn test_apply_full_pipeline_with_blend() {
    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        return;
    }

    let input_cpu = load_lena_rgba_u8();
    let overlay_cpu = input_cpu.clone();
    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;
    let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);

    let mut main_buffer = ImageBuffer::from_cpu(input_cpu.clone());
    let mut temp_buffer = ImageBuffer::new_empty(*input_cpu.desc());
    let mut overlay_buffer = ImageBuffer::from_cpu(overlay_cpu);
    let mut blend_output = ImageBuffer::new_empty(*input_cpu.desc());

    // Step 1: Rotate main image
    Transform::default()
        .rotate_around(PI / 8.0, center)
        .execute(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut main_buffer, &mut temp_buffer);

    // Step 2: Adjust contrast
    ContrastBrightness::default()
        .contrast(1.2)
        .execute(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut main_buffer, &mut temp_buffer);

    // Step 3: Rotate overlay differently
    Transform::default()
        .rotate_around(-PI / 8.0, center)
        .execute(&mut ctx, &overlay_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut overlay_buffer, &mut temp_buffer);

    // Step 4: Blend main with overlay
    Blend::default()
        .mode(BlendMode::Screen)
        .alpha(0.5)
        .execute(&mut ctx, &overlay_buffer, &main_buffer, &mut blend_output)
        .unwrap();

    // Step 5: Final contrast adjustment
    ContrastBrightness::default()
        .brightness(0.05)
        .execute(&mut ctx, &blend_output, &mut temp_buffer)
        .unwrap();

    let result = temp_buffer.make_cpu(&ctx).unwrap();
    assert_eq!(result.desc().width, width);
    assert_eq!(result.desc().height, height);
}

#[test]
fn test_apply_error_on_mismatched_formats() {
    let mut ctx = ProcessingContext::new();

    // Create images with different formats
    let img_rgba = load_lena_rgba_u8();
    let img_rgb = img_rgba.clone().convert(ColorFormat::RGB_U8).unwrap();

    let input = ImageBuffer::from_cpu(img_rgba);
    let mut output = ImageBuffer::from_cpu(img_rgb);

    // apply() should return error due to format mismatch
    let contrast = ContrastBrightness::default().contrast(1.2);
    let result = contrast.execute(&mut ctx, &input, &mut output);

    assert!(result.is_err());
}
