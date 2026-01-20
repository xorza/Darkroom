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

    let mut main_buffer = ImageBuffer::from_cpu(input_cpu.clone());
    let mut temp_buffer = ImageBuffer::new_empty(*input_cpu.desc());
    let mut overlay_buffer = ImageBuffer::from_cpu(input_cpu.clone());
    let mut blend_output = ImageBuffer::new_empty(*input_cpu.desc());

    // Step 1: Rotate main image
    println!("Step 1: Rotating main image...");
    Transform::default()
        .rotate_around(PI / 8.0, center)
        .execute(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut main_buffer, &mut temp_buffer);

    // Step 2: Adjust contrast
    println!("Step 2: Adjusting contrast...");
    ContrastBrightness::default()
        .contrast(1.2)
        .execute(&mut ctx, &main_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut main_buffer, &mut temp_buffer);

    // Step 3: Rotate overlay differently
    println!("Step 3: Rotating overlay...");
    Transform::default()
        .rotate_around(-PI / 8.0, center)
        .execute(&mut ctx, &overlay_buffer, &mut temp_buffer)
        .unwrap();
    std::mem::swap(&mut overlay_buffer, &mut temp_buffer);

    // Step 4: Blend main with overlay
    println!("Step 4: Blending images...");
    Blend::default()
        .mode(BlendMode::Screen)
        .alpha(0.5)
        .execute(&mut ctx, &overlay_buffer, &main_buffer, &mut blend_output)
        .unwrap();

    // Step 5: Final brightness adjustment
    println!("Step 5: Final brightness adjustment...");
    ContrastBrightness::default()
        .brightness(0.05)
        .execute(&mut ctx, &blend_output, &mut temp_buffer)
        .unwrap();

    let result = temp_buffer.make_cpu(&ctx).unwrap();
    save_image(&result, "pipeline_final.png");

    println!("Done! Pipeline completed.");
}
