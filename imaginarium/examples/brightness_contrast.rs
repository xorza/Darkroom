mod common;

use common::*;
use imaginarium::prelude::*;

fn main() {
    ensure_output_dir();

    let input = load_lena_rgba_u8();
    print_image_info("Input", &input);

    let mut output = Image::new_black(*input.desc()).unwrap();

    // CPU example
    ContrastBrightness::new(1.5, 0.1).apply_cpu(&input, &mut output);
    save_image(&output, "contrast_brightness_cpu.png");

    // GPU example
    let mut ctx = ProcessingContext::new();
    let input_buf = ImageBuffer::from_cpu(input);
    let mut output_buf = ImageBuffer::new_empty(*input_buf.desc());

    ContrastBrightness::new(1.5, 0.1)
        .execute(&mut ctx, &input_buf, &mut output_buf)
        .unwrap();

    let result = output_buf.make_cpu(&ctx).unwrap();
    save_image(&result, "contrast_brightness_gpu.png");
}
