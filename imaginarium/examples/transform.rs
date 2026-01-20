mod common;

use std::f32::consts::PI;

use common::*;
use imaginarium::prelude::*;

fn main() {
    ensure_output_dir();

    let ctx = Gpu::new().expect("Failed to create GPU context");
    let pipeline = GpuTransformPipeline::new(&ctx).expect("Failed to create transform pipeline");

    let input_cpu = load_lena_rgba_u8();
    print_image_info("Input", &input_cpu);

    let input = GpuImage::from_image(&ctx, &input_cpu);

    let center = Vec2::new(
        input_cpu.desc().width as f32 / 2.0,
        input_cpu.desc().height as f32 / 2.0,
    );

    // Scale down 50%
    let mut output = GpuImage::new_empty(
        &ctx,
        ImageDesc::new(256, 256, input_cpu.desc().color_format),
    );
    Transform::new()
        .scale(Vec2::new(0.5, 0.5))
        .apply_gpu(&ctx, &pipeline, &input, &mut output);
    let output_cpu = output.to_image(&ctx).unwrap();
    save_image(&output_cpu, "transform_scale_down.png");

    // Scale up 150%
    let mut output = GpuImage::new_empty(
        &ctx,
        ImageDesc::new(768, 768, input_cpu.desc().color_format),
    );
    Transform::new()
        .scale(Vec2::new(1.5, 1.5))
        .apply_gpu(&ctx, &pipeline, &input, &mut output);
    let output_cpu = output.to_image(&ctx).unwrap();
    save_image(&output_cpu, "transform_scale_up.png");

    // Rotate 45 degrees around center
    let mut output = GpuImage::new_empty(&ctx, *input_cpu.desc());
    Transform::new().rotate_around(PI / 4.0, center).apply_gpu(
        &ctx,
        &pipeline,
        &input,
        &mut output,
    );
    let output_cpu = output.to_image(&ctx).unwrap();
    save_image(&output_cpu, "transform_rotate_45.png");

    // Rotate 90 degrees
    let mut output = GpuImage::new_empty(&ctx, *input_cpu.desc());
    Transform::new().rotate_around(PI / 2.0, center).apply_gpu(
        &ctx,
        &pipeline,
        &input,
        &mut output,
    );
    let output_cpu = output.to_image(&ctx).unwrap();
    save_image(&output_cpu, "transform_rotate_90.png");

    // Combined: scale + rotate + translate
    let mut output = GpuImage::new_empty(
        &ctx,
        ImageDesc::new(800, 600, input_cpu.desc().color_format),
    );
    Transform::new()
        .scale(Vec2::new(0.7, 0.7))
        .rotate(PI / 6.0) // 30 degrees
        .translate(Vec2::new(200.0, 100.0))
        .filter(FilterMode::Bilinear)
        .apply_gpu(&ctx, &pipeline, &input, &mut output);
    let output_cpu = output.to_image(&ctx).unwrap();
    save_image(&output_cpu, "transform_combined.png");

    // Multiple transforms applied in chain (ping-pong between GPU buffers)
    let mut buffer_a = input.clone_buffer(&ctx);
    let mut buffer_b = input.clone_buffer(&ctx);

    // First pass: rotate 30 degrees
    Transform::new().rotate_around(PI / 6.0, center).apply_gpu(
        &ctx,
        &pipeline,
        &buffer_a,
        &mut buffer_b,
    );

    // Second pass: scale down slightly
    Transform::new()
        .scale(Vec2::new(0.9, 0.9))
        .translate(center * 0.1)
        .apply_gpu(&ctx, &pipeline, &buffer_b, &mut buffer_a);

    // Third pass: rotate another 30 degrees
    Transform::new().rotate_around(PI / 6.0, center).apply_gpu(
        &ctx,
        &pipeline,
        &buffer_a,
        &mut buffer_b,
    );

    // Fourth pass: scale down again
    Transform::new()
        .scale(Vec2::new(0.9, 0.9))
        .translate(center * 0.1)
        .apply_gpu(&ctx, &pipeline, &buffer_b, &mut buffer_a);

    // Download final result
    let output_cpu = buffer_a.to_image(&ctx).unwrap();
    save_image(&output_cpu, "transform_multi_pass.png");

    // Compare filter modes: nearest vs bilinear on upscale
    let mut output_nearest = GpuImage::new_empty(
        &ctx,
        ImageDesc::new(1024, 1024, input_cpu.desc().color_format),
    );
    Transform::new()
        .scale(Vec2::new(2.0, 2.0))
        .filter(FilterMode::Nearest)
        .apply_gpu(&ctx, &pipeline, &input, &mut output_nearest);
    let output_cpu = output_nearest.to_image(&ctx).unwrap();
    save_image(&output_cpu, "transform_upscale_nearest.png");

    let mut output_bilinear = GpuImage::new_empty(
        &ctx,
        ImageDesc::new(1024, 1024, input_cpu.desc().color_format),
    );
    Transform::new()
        .scale(Vec2::new(2.0, 2.0))
        .filter(FilterMode::Bilinear)
        .apply_gpu(&ctx, &pipeline, &input, &mut output_bilinear);
    let output_cpu = output_bilinear.to_image(&ctx).unwrap();
    save_image(&output_cpu, "transform_upscale_bilinear.png");

    println!("Done!");
}
