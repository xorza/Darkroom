mod common;

use common::*;
use imaginarium::*;

fn main() {
    ensure_output_dir();

    let input = load_lena_rgba_u8();
    print_image_info("Input", &input);

    // CPU example — the op is in-place, so adjust a copy of the input.
    let mut output = input.clone();
    ContrastBrightness::new(1.5, 0.1).apply_cpu(&mut output);
    save_image(&output, "contrast_brightness_cpu.png");

    // GPU example
    let mut ctx = ProcessingContext::new();
    let mut buffer = ImageBuffer::from_cpu(input);

    ContrastBrightness::new(1.5, 0.1)
        .execute(&mut ctx, &mut buffer)
        .unwrap();

    let result = buffer.make_cpu(&ctx).unwrap();
    save_image(&result, "contrast_brightness_gpu.png");
}
