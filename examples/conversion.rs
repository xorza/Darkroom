mod common;

use common::{ensure_output_dir, load_lena_rgba_u8, print_image_info, save_image};
use imaginarium::ColorFormat;

fn main() {
    ensure_output_dir();

    let input = load_lena_rgba_u8();
    print_image_info("Input", &input);

    let gray = input.convert_to(ColorFormat::L_U8);
    save_image(&gray, "gray_u8.png");

    let u16_img = input.convert_to(ColorFormat::RGBA_U16);
    print_image_info("Unsigned 16-bit", &u16_img);
    save_image(&u16_img, "rgba_u16.tiff");

    let f32_img = input.convert_to(ColorFormat::RGBA_F32);
    save_image(&f32_img, "rgba_f32.tiff");

    let rgba = input
        .convert_to(ColorFormat::RGB_U8)
        .convert(ColorFormat::RGBA_U8);
    save_image(&rgba, "rgb_to_rgba.png");

    let roundtrip = input
        .convert(ColorFormat::RGBA_F32)
        .convert(ColorFormat::RGBA_U8);
    save_image(&roundtrip, "roundtrip_f32_u8.png");

    let gray_to_rgb = gray.convert(ColorFormat::RGB_U8);
    save_image(&gray_to_rgb, "gray_to_rgb.png");

    let back_to_u8 = u16_img.convert(ColorFormat::RGBA_U8);
    save_image(&back_to_u8, "u16_to_u8.png");
}
