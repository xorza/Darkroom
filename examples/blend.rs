#![cfg_attr(
    feature = "wgpu",
    expect(
        clippy::print_stdout,
        reason = "an example reports what it did on stdout"
    )
)]

mod common;

use common::{ensure_output_dir, load_lena_rgba_u8, print_image_info, save_image};
use imaginarium::{Blend, BlendMode, Image};

fn main() {
    ensure_output_dir();

    let src = load_lena_rgba_u8();
    let dst = load_lena_rgba_u8();
    print_image_info("Source", &src);

    // CPU
    let mut output = Image::new_black(src.desc()).unwrap();
    Blend::new(BlendMode::Screen, 0.5).apply_cpu(&src, &dst, &mut output);
    save_image(&output, "blend_cpu.png");

    #[cfg(feature = "wgpu")]
    on_gpu(&src, &dst);
}

/// Upload both inputs, allocate the output on the device, run, download.
#[cfg(feature = "wgpu")]
fn on_gpu(src: &Image, dst: &Image) {
    use imaginarium::{Gpu, GpuBlendPipeline, GpuContext, GpuImage};

    let Ok(gpu) = Gpu::new() else {
        println!("no GPU available, skipping the GPU example");
        return;
    };
    let mut context = GpuContext::new(gpu.clone());
    let pipeline = context.get_or_create(GpuBlendPipeline::new);

    let src_gpu = GpuImage::from_image(&gpu, src).expect("upload");
    let dst_gpu = GpuImage::from_image(&gpu, dst).expect("upload");
    let mut output_gpu = GpuImage::new_empty(&gpu, src.desc()).expect("allocate");
    Blend::new(BlendMode::Screen, 0.5).apply_gpu(
        &gpu,
        pipeline,
        &src_gpu,
        &dst_gpu,
        &mut output_gpu,
    );

    save_image(&output_gpu.to_image(&gpu).unwrap(), "blend_gpu.png");
}
