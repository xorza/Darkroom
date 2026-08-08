mod common;

use common::*;
use imaginarium::*;

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
    let Ok(gpu) = Gpu::new() else {
        println!("no GPU available, skipping the GPU example");
        return;
    };
    let mut context = GpuContext::new(gpu.clone());
    let pipeline = context.get_or_create(GpuBlendPipeline::new).unwrap();

    let src_gpu = GpuImage::from_image(&gpu, src);
    let dst_gpu = GpuImage::from_image(&gpu, dst);
    let mut output_gpu = GpuImage::new_empty(&gpu, src.desc());
    Blend::new(BlendMode::Screen, 0.5)
        .apply_gpu(&gpu, pipeline, &src_gpu, &dst_gpu, &mut output_gpu)
        .unwrap();

    save_image(&output_gpu.to_image(&gpu).unwrap(), "blend_gpu.png");
}
