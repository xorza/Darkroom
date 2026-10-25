#![cfg_attr(
    feature = "wgpu",
    expect(
        clippy::print_stdout,
        reason = "an example reports what it did on stdout"
    )
)]

mod shared;

use imaginarium::ContrastBrightness;
#[cfg(feature = "wgpu")]
use imaginarium::Image;
use shared::{load_lena_rgba_u8, print_image_info, save_image};

fn main() {
    let input = load_lena_rgba_u8();
    print_image_info("Input", &input);

    #[cfg(feature = "wgpu")]
    on_gpu(&input);

    // CPU: the op is in place, so the input becomes the output.
    let mut output = input;
    ContrastBrightness::new(1.5, 0.1).apply_cpu(&mut output);
    save_image(&output, "contrast_brightness_cpu.png");
}

/// Residency is the caller's to manage: upload, run, download. The shader reads and writes through
/// separate bindings, so the op needs a distinct output image.
#[cfg(feature = "wgpu")]
fn on_gpu(input: &Image) {
    use imaginarium::{Gpu, GpuContext, GpuContrastBrightnessPipeline, GpuImage};

    let Ok(gpu) = Gpu::new() else {
        println!("no GPU available, skipping the GPU example");
        return;
    };
    let mut context = GpuContext::new(gpu.clone());
    let pipeline = context.get_or_create(GpuContrastBrightnessPipeline::new);

    let uploaded = GpuImage::from_image(&gpu, input).expect("upload");
    let mut rendered = GpuImage::new_empty(&gpu, input.desc()).expect("allocate");
    ContrastBrightness::new(1.5, 0.1).apply_gpu(&gpu, pipeline, &uploaded, &mut rendered);

    save_image(
        &rendered.to_image(&gpu).unwrap(),
        "contrast_brightness_gpu.png",
    );
}
