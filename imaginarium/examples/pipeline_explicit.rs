mod common;

use std::f32::consts::PI;

use common::*;
use imaginarium::prelude::*;

fn main() {
    ensure_output_dir();

    let mut ctx = ProcessingContext::new();
    if !ctx.has_gpu() {
        println!("No GPU available, exiting");
        return;
    }

    let input_cpu = load_lena_rgba_u8();
    print_image_info("Input", &input_cpu);

    let width = input_cpu.desc().width;
    let height = input_cpu.desc().height;
    let center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);

    // Create buffers
    let mut main_buffer = ImageBuffer::from_cpu(input_cpu.clone());
    let mut temp_buffer = ImageBuffer::new_empty(*input_cpu.desc());

    // Step 1: Rotate 15 degrees (GPU)
    println!("Step 1: Rotating 15 degrees...");
    let transform = Transform::default().rotate_around(PI / 12.0, center);
    transform
        .execute_gpu(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut main_buffer, &mut temp_buffer);

    // Save intermediate result
    main_buffer.make_cpu(&ctx).unwrap();
    save_image(
        &main_buffer.make_cpu(&ctx).unwrap(),
        "pipeline_step1_rotate.png",
    );

    // Step 2: Increase contrast (CPU)
    println!("Step 2: Adjusting contrast...");
    let contrast = ContrastBrightness::default().contrast(1.3);
    contrast
        .execute_cpu(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut main_buffer, &mut temp_buffer);

    save_image(
        &main_buffer.make_cpu(&ctx).unwrap(),
        "pipeline_step2_contrast.png",
    );

    // Step 3: Scale down (GPU)
    println!("Step 3: Scaling down...");
    let transform = Transform::default()
        .scale(Vec2::new(0.8, 0.8))
        .translate(Vec2::new(width as f32 * 0.1, height as f32 * 0.1));
    transform
        .execute_gpu(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut main_buffer, &mut temp_buffer);

    main_buffer.make_cpu(&ctx).unwrap();
    save_image(
        &main_buffer.make_cpu(&ctx).unwrap(),
        "pipeline_step3_scale.png",
    );

    // Step 4: Adjust brightness (CPU)
    println!("Step 4: Adjusting brightness...");
    let brightness = ContrastBrightness::default().brightness(0.1);
    brightness
        .execute_cpu(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut main_buffer, &mut temp_buffer);

    save_image(
        &main_buffer.make_cpu(&ctx).unwrap(),
        "pipeline_step4_brightness.png",
    );

    // Step 5: Create overlay effect - blend with rotated version
    println!("Step 5: Creating overlay blend...");
    let mut overlay_buffer = ImageBuffer::from_cpu(input_cpu.clone());
    let mut overlay_temp = ImageBuffer::new_empty(*input_cpu.desc());

    // Rotate overlay 30 degrees the other way
    let transform = Transform::default().rotate_around(-PI / 6.0, center);
    transform
        .execute_gpu(&mut ctx, &overlay_buffer, &mut overlay_temp)
        .unwrap();
    std::mem::swap(&mut overlay_buffer, &mut overlay_temp);
    overlay_buffer.make_cpu(&ctx).unwrap();

    let mut blend_output = ImageBuffer::new_empty(*input_cpu.desc());
    let blend = Blend::default().mode(BlendMode::Screen).alpha(0.4);
    blend
        .execute_gpu(&mut ctx, &overlay_buffer, &main_buffer, &mut blend_output)
        .unwrap();

    blend_output.make_cpu(&ctx).unwrap();
    save_image(
        &blend_output.make_cpu(&ctx).unwrap(),
        "pipeline_step5_blend.png",
    );

    // Step 6: Final rotation (GPU)
    println!("Step 6: Final rotation...");
    let mut final_output = ImageBuffer::new_empty(*input_cpu.desc());
    let transform = Transform::default().rotate_around(PI / 24.0, center);
    transform
        .execute_gpu(&mut ctx, &blend_output, &mut final_output)
        .unwrap();

    let final_result = final_output.make_cpu(&ctx).unwrap();
    save_image(&final_result, "pipeline_final.png");

    println!("Done! Pipeline completed with 6 chained operations.");
}
