use crate::common::color_format::{ALL_FORMATS, ColorFormat};
use crate::common::internals::create_test_image;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;
use crate::ops::preview::Preview;

fn img(w: usize, h: usize, fmt: ColorFormat, bytes: Vec<u8>) -> Image {
    Image::new_with_data(ImageDesc::new(w, h, fmt), bytes).unwrap()
}

/// Same-size "downscale" is a per-pixel convert: each footprint is one pixel,
/// so an `RGBA_U8` source round-trips byte-exact.
#[test]
fn test_same_size_is_identity_rgba8() {
    let src = img(
        2,
        2,
        ColorFormat::RGBA_U8,
        vec![
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
        ],
    );
    let out = Preview::new(2, 2).to_rgba8(&src);
    assert_eq!(out.desc(), src.desc());
    assert_eq!(out.bytes(), src.bytes());
}

/// 2×2 → 1×1 averages all four pixels. R: (0+100+200+40)/4 = 85, etc.; the
/// opaque alphas average back to 255.
#[test]
fn test_box_average_2x_to_1x() {
    let src = img(
        2,
        2,
        ColorFormat::RGBA_U8,
        vec![
            0, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255, 40, 40, 40, 255,
        ],
    );
    let out = Preview::new(1, 1).to_rgba8(&src);
    assert_eq!(out.desc().width, 1);
    assert_eq!(out.desc().height, 1);
    assert_eq!(out.bytes(), &[85, 85, 85, 255]);
}

/// Grayscale broadcasts to RGB with an opaque alpha. L8 [0,100,200,40] → 1×1
/// gray 85 → (85,85,85,255).
#[test]
fn test_l8_broadcasts_with_opaque_alpha() {
    let src = img(2, 2, ColorFormat::L_U8, vec![0, 100, 200, 40]);
    let out = Preview::new(1, 1).to_rgba8(&src);
    assert_eq!(out.bytes(), &[85, 85, 85, 255]);
}

/// RGB source gains an opaque alpha; a single 1×1 RGB pixel passes through.
#[test]
fn test_rgb8_gains_opaque_alpha() {
    let src = img(1, 1, ColorFormat::RGB_U8, vec![12, 34, 56]);
    let out = Preview::new(1, 1).to_rgba8(&src);
    assert_eq!(out.bytes(), &[12, 34, 56, 255]);
}

/// U16 is rescaled to 8-bit: 65535 → 255, and 32768 (just over half of the
/// 65535 max) → round(32768·255/65535) = round(127.502) = 128 — what the format
/// conversion gives.
#[test]
fn test_u16_rescaled_to_u8() {
    let px: Vec<u8> = [65535u16, 32768, 0, 65535]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let src = img(1, 1, ColorFormat::RGBA_U16, px);
    let out = Preview::new(1, 1).to_rgba8(&src);
    assert_eq!(out.bytes(), &[255, 128, 0, 255]);
}

/// F32 in [0,1] maps ×255 with rounding: 1.0→255, 0.0→0, 0.2→51, and the
/// missing alpha is opaque (`RGB_F32` source).
#[test]
fn test_f32_scaled_to_u8() {
    let px: Vec<u8> = [1.0f32, 0.0, 0.2]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let src = img(1, 1, ColorFormat::RGB_F32, px);
    let out = Preview::new(1, 1).to_rgba8(&src);
    assert_eq!(out.bytes(), &[255, 0, 51, 255]);
}

/// Non-integer reduction (3→2) tiles the source by integer floor into
/// footprints {0} and {1,2}: out0 = 0, out1 = avg(60,120) = 90.
#[test]
fn test_non_integer_downscale_footprints() {
    let src = img(3, 1, ColorFormat::L_U8, vec![0, 60, 120]);
    let out = Preview::new(2, 1).to_rgba8(&src);
    assert_eq!(out.bytes(), &[0, 0, 0, 255, 90, 90, 90, 255]);
}

/// A footprint mean on a tie rounds to even: (1 + 2) / 2 = 1.5 → 2,
/// (2 + 3) / 2 = 2.5 → 2. In `u16`, (257 + 514) / 2 / 257 = 1.5 → 2 as well.
#[test]
fn test_footprint_mean_ties_to_even() {
    let src = img(4, 1, ColorFormat::L_U8, vec![1, 2, 2, 3]);
    let out = Preview::new(2, 1).to_rgba8(&src);
    assert_eq!(out.bytes(), &[2, 2, 2, 255, 2, 2, 2, 255]);

    let words: Vec<u8> = [257u16, 514].iter().flat_map(|v| v.to_le_bytes()).collect();
    let src = img(2, 1, ColorFormat::L_U16, words);
    assert_eq!(Preview::new(1, 1).to_rgba8(&src).bytes(), &[2, 2, 2, 255]);
}

/// A one-pixel footprint is the format conversion to `RGBA_U8`, for every format.
#[test]
fn test_same_size_is_the_conversion() {
    for format in ALL_FORMATS {
        let src = create_test_image(format, 7, 3, 5);
        let out = Preview::new(7, 3).to_rgba8(&src);
        assert_eq!(
            out.bytes(),
            src.convert_to(ColorFormat::RGBA_U8).bytes(),
            "{format}"
        );
    }
}

/// Every format reduces to a valid, correctly-sized `RGBA_U8` preview.
#[test]
fn test_all_formats_to_rgba8() {
    for format in ALL_FORMATS {
        let src = create_test_image(format, 17, 11, 3);
        let out = Preview::new(8, 5).to_rgba8(&src);
        assert_eq!(out.desc().color_format, ColorFormat::RGBA_U8);
        assert_eq!(out.desc().width, 8);
        assert_eq!(out.desc().height, 5);
        assert_eq!(out.bytes().len(), 8 * 5 * 4);
    }
}
