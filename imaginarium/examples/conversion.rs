mod shared;

use imaginarium::ColorFormat;
use shared::{load_lena_rgba_u8, print_image_info, save_image};

fn main() {
    let input = load_lena_rgba_u8();
    print_image_info("Input", &input);

    let mono = input.convert_to(ColorFormat::MONO_U8);
    save_image(&mono, "mono_u8.png");

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

    let mono_to_rgb = mono.convert(ColorFormat::RGB_U8);
    save_image(&mono_to_rgb, "mono_to_rgb.png");

    let back_to_u8 = u16_img.convert(ColorFormat::RGBA_U8);
    save_image(&back_to_u8, "u16_to_u8.png");
}
