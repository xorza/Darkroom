//! A multi-step pipeline, showing what callers own, since the ops manage no residency: upload
//! once, run a chain of GPU ops without ever going back to the CPU, and read back once at the
//! end. Keeping the data on the device across steps is the caller's choice — nothing does it
//! automatically — and step 2 shows the round trip you pay when you *do* need the CPU.

#![expect(
    clippy::print_stdout,
    reason = "an example reports what it did on stdout"
)]
mod shared;

use std::f32::consts::PI;
use std::mem;

use glam::Vec2;
use imaginarium::{AffineWarp, Blend, BlendMode, ContrastBrightness, Gpu, GpuImage};
use shared::{load_lena_rgba_u8, print_image_info, save_image};

fn main() {
    let Ok(gpu) = Gpu::new() else {
        println!("No GPU available, exiting");
        return;
    };

    let input = load_lena_rgba_u8();
    print_image_info("Input", &input);

    let desc = input.desc();
    let center = Vec2::new(
        desc.size().width as f32 / 2.0,
        desc.size().height as f32 / 2.0,
    );

    let mut main = GpuImage::from_image(&gpu, &input).expect("upload");
    let mut scratch = GpuImage::new_empty(&gpu, desc).expect("allocate");

    // Step 1: rotate. The transform shader cannot read and write one image, so it renders into
    // `scratch` and the two swap — `main` is always the live one.
    println!("Step 1: rotating...");
    AffineWarp::default()
        .rotate_around(PI / 12.0, center)
        .apply_gpu(&gpu, &main, &mut scratch);
    mem::swap(&mut main, &mut scratch);

    // Step 2: contrast, on the CPU. This is the one step that costs a round trip, and it is
    // visible in the code rather than hidden behind a backend chooser.
    println!("Step 2: adjusting contrast (CPU)...");
    let mut cpu = main.to_image(&gpu).unwrap();
    ContrastBrightness::default()
        .contrast(1.3)
        .apply_cpu(&mut cpu);
    main = GpuImage::from_image(&gpu, &cpu).expect("upload");

    // Step 3: blend against a differently-rotated copy of the original, all on the device.
    println!("Step 3: blending an overlay...");
    let mut overlay = GpuImage::from_image(&gpu, &input).expect("upload");
    AffineWarp::default()
        .rotate_around(-PI / 6.0, center)
        .apply_gpu(&gpu, &overlay, &mut scratch);
    mem::swap(&mut overlay, &mut scratch);

    let mut blended = GpuImage::new_empty(&gpu, desc).expect("allocate");
    Blend::default()
        .mode(BlendMode::Screen)
        .alpha(0.4)
        .apply_gpu(&gpu, &overlay, &main, &mut blended);

    // Read back once, at the end.
    let result = blended.to_image(&gpu).unwrap();
    save_image(&result, "pipeline_final.png");

    println!("Done! Pipeline completed.");
}
