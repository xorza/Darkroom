mod common;

use common::*;
use imaginarium::prelude::*;

fn main() {
    ensure_output_dir();

    let input = load_lena_rgba_u8();
    print_image_info("Input", &input);

    // Convert to grayscale (averages RGB channels)
    let gray = input.clone().convert(ColorFormat::L_U8).unwrap();
    save_image(&gray, "gray_u8.png");

    // Convert to 16-bit (bit replication for full range)
    let u16_img = input.clone().convert(ColorFormat::RGBA_U16).unwrap();
    save_image(&u16_img, "rgba_u16.tiff");

    // Convert to floating point (normalizes to 0.0-1.0)
    let f32_img = input.clone().convert(ColorFormat::RGBA_F32).unwrap();
    save_image(&f32_img, "rgba_f32.tiff");

    // Convert RGB to RGBA (adds max alpha)
    let rgb = input.clone().convert(ColorFormat::RGB_U8).unwrap();
    let rgba = rgb.convert(ColorFormat::RGBA_U8).unwrap();
    save_image(&rgba, "rgb_to_rgba.png");

    // Chain conversions: load -> float -> back to u8
    let roundtrip = load_lena_rgba_u8()
        .convert(ColorFormat::RGBA_F32)
        .unwrap()
        .convert(ColorFormat::RGBA_U8)
        .unwrap();
    save_image(&roundtrip, "roundtrip_f32_u8.png");

    // Convert grayscale back to RGB (replicates gray to all channels)
    let gray_to_rgb = gray.convert(ColorFormat::RGB_U8).unwrap();
    save_image(&gray_to_rgb, "gray_to_rgb.png");

    // 16-bit unsigned integer conversion
    let u16_img = input.clone().convert(ColorFormat::RGBA_U16).unwrap();
    print_image_info("Unsigned 16-bit", &u16_img);
    let back_to_u8 = u16_img.convert(ColorFormat::RGBA_U8).unwrap();
    save_image(&back_to_u8, "u16_to_u8.png");
}
