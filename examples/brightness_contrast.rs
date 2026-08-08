mod common;

use common::*;
use imaginarium::*;

fn main() {
    ensure_output_dir();

    let input = load_lena_rgba_u8();
    print_image_info("Input", &input);

    // CPU: the op is in-place, so adjust a copy of the input.
    let mut output = input.clone();
    ContrastBrightness::new(1.5, 0.1).apply_cpu(&mut output);
    save_image(&output, "contrast_brightness_cpu.png");

    #[cfg(feature = "wgpu")]
    on_gpu(&input);
}

/// Residency is the caller's to manage: upload, run, download. The shader reads and writes through
/// separate bindings, so the op needs a distinct output image.
#[cfg(feature = "wgpu")]
fn on_gpu(input: &Image) {
    let Ok(gpu) = Gpu::new() else {
        println!("no GPU available, skipping the GPU example");
        return;
    };
    let mut context = GpuContext::new(gpu.clone());
    let pipeline = context
        .get_or_create(GpuContrastBrightnessPipeline::new)
        .unwrap();

    let uploaded = GpuImage::from_image(&gpu, input);
    let mut rendered = GpuImage::new_empty(&gpu, input.desc());
    ContrastBrightness::new(1.5, 0.1)
        .apply_gpu(&gpu, pipeline, &uploaded, &mut rendered)
        .unwrap();

    save_image(
        &rendered.to_image(&gpu).unwrap(),
        "contrast_brightness_gpu.png",
    );
}
