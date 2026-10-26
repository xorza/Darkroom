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
use imaginarium::InterleavedImage;
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

/// Residency is the caller's to manage: upload, run, read back. The op runs in place on the
/// upload, as it does on the CPU.
#[cfg(feature = "wgpu")]
fn on_gpu(input: &InterleavedImage) {
    use imaginarium::{Gpu, GpuImage};

    let Ok(gpu) = Gpu::new() else {
        println!("no GPU available, skipping the GPU example");
        return;
    };

    let mut rendered = GpuImage::from_image(&gpu, input).expect("upload");
    ContrastBrightness::new(1.5, 0.1).apply_gpu(&gpu, &mut rendered);

    save_image(
        &rendered.to_image(&gpu).unwrap(),
        "contrast_brightness_gpu.png",
    );
}
