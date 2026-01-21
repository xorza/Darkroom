mod common;

use common::*;
use imaginarium::prelude::*;

fn main() {
    ensure_output_dir();

    let src = load_lena_rgba_u8();
    let dst = load_lena_rgba_u8();
    print_image_info("Source", &src);

    let mut output = Image::new_black(*src.desc()).unwrap();

    // CPU example
    Blend::new(BlendMode::Screen, 0.5).apply_cpu(&src, &dst, &mut output);
    save_image(&output, "blend_cpu.png");

    // GPU example
    let mut ctx = ProcessingContext::new();
    let src_buf = ImageBuffer::from_cpu(src);
    let dst_buf = ImageBuffer::from_cpu(dst);
    let mut output_buf = ImageBuffer::new_empty(*src_buf.desc());

    Blend::new(BlendMode::Screen, 0.5)
        .execute(&mut ctx, &src_buf, &dst_buf, &mut output_buf)
        .unwrap();

    let result = output_buf.make_cpu(&ctx).unwrap();
    save_image(&result, "blend_gpu.png");
}
