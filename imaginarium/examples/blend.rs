#![cfg_attr(
    feature = "wgpu",
    expect(
        clippy::print_stdout,
        reason = "an example reports what it did on stdout"
    )
)]

mod shared;

use imaginarium::{Blend, BlendMode, InterleavedImage};
use shared::{load_lena_rgba_u8, print_image_info, save_image};

fn main() {
    let src = load_lena_rgba_u8();
    let dst = load_lena_rgba_u8();
    print_image_info("Source", &src);

    let mut output = InterleavedImage::new_black(src.desc());
    Blend::new(BlendMode::Screen, 0.5).apply_cpu(&src, &dst, &mut output);
    save_image(&output, "blend_cpu.png");

    #[cfg(feature = "wgpu")]
    on_gpu(&src, &dst);
}

/// Upload both inputs, allocate the output on the device, run, read back.
#[cfg(feature = "wgpu")]
fn on_gpu(src: &InterleavedImage, dst: &InterleavedImage) {
    use imaginarium::{Gpu, GpuBlendPipeline, GpuCtx, GpuImage};

    let Ok(gpu) = Gpu::new() else {
        println!("no GPU available, skipping the GPU example");
        return;
    };
    let mut context = GpuCtx::new(gpu.clone());
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
