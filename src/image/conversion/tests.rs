use crate::common::color_format::{ALL_FORMATS, ColorFormat};
use crate::image::conversion::scalar::{self, Luminance};
use crate::image::{Image, ImageDesc};

/// `(r·13933 + g·46871 + b·4732) / 65536`, rounded half to even:
/// red 255 → 3 552 915 / 65536 = 54.21 → 54; green 255 → 11 952 105 / 65536 = 182.38 → 182;
/// blue 255 → 1 206 660 / 65536 = 18.41 → 18; grey 255 → 255 exactly. In `u16` the weights are
/// the same, so red 65535 → 913 099 155 / 65536 = 13 932.79 → 13 933. Float carries the weights
/// unrounded.
#[test]
fn luminance_is_the_rounded_rec709_sum() {
    for (rgb, expected) in [
        ([255, 0, 0], 54),
        ([0, 255, 0], 182),
        ([0, 0, 255], 18),
        ([255, 255, 255], 255),
        ([0, 0, 0], 0),
    ] {
        assert_eq!(u8::luminance(rgb[0], rgb[1], rgb[2]), expected, "{rgb:?}");
    }
    assert_eq!(u16::luminance(65535, 0, 0), 13933);
    assert_eq!(u16::luminance(65535, 65535, 65535), 65535);
    assert_eq!(f32::luminance(1.0, 0.0, 0.0), 0.2126);
}

/// Exhaustive over `u8` grey: every grey value is its own luminance, which needs the weights to
/// sum to 65536 and the rounding to be exact.
#[test]
fn grey_keeps_its_value() {
    for v in 0..=u8::MAX {
        assert_eq!(u8::luminance(v, v, v), v);
    }
}

/// One pixel per channel-count pair, in `u8`: grey broadcasts and gains an opaque alpha, colour
/// keeps its channels or reduces to luminance, alpha is kept only into a format with alpha.
#[test]
fn channel_counts_expand_and_reduce() {
    let l = [100u8];
    let rgb = [255u8, 0, 0];
    let rgba = [255u8, 0, 0, 7];
    for (from, src, to, expected) in [
        (
            ColorFormat::L_U8,
            &l[..],
            ColorFormat::RGB_U8,
            &[100, 100, 100][..],
        ),
        (
            ColorFormat::L_U8,
            &l,
            ColorFormat::RGBA_U8,
            &[100, 100, 100, 255],
        ),
        (ColorFormat::RGB_U8, &rgb, ColorFormat::L_U8, &[54]),
        (
            ColorFormat::RGB_U8,
            &rgb,
            ColorFormat::RGBA_U8,
            &[255, 0, 0, 255],
        ),
        (ColorFormat::RGBA_U8, &rgba, ColorFormat::L_U8, &[54]),
        (
            ColorFormat::RGBA_U8,
            &rgba,
            ColorFormat::RGB_U8,
            &[255, 0, 0],
        ),
    ] {
        let mut dst = vec![0; to.byte_count()];
        scalar::row_converter(from, to)(src, &mut dst, 1);
        assert_eq!(dst, expected, "{from} -> {to}");
    }
}

/// Literals, not the code under test: `u8 → u16` is `v · 257`, so 64 → 16448, 128 → 32896,
/// 200 → 51400; `u16 → u8` rounds `v / 257`, so 32896 → 128 and 32767 → 127.5 − ε → 127;
/// `f32 → u8` rounds `x · 255`, so 0.5 → 127.5 → 128 (ties to even) and 0.2 → 51; the alpha
/// a format gains is opaque.
#[test]
fn whole_images_convert_by_the_rule() {
    let cases: [(ColorFormat, Vec<u8>, ColorFormat, Vec<u8>); 4] = [
        (
            ColorFormat::L_U8,
            vec![0, 64, 128, 200, 255],
            ColorFormat::L_U16,
            bytemuck::cast_slice(&[0u16, 16448, 32896, 51400, 65535]).to_vec(),
        ),
        (
            ColorFormat::L_U16,
            bytemuck::cast_slice(&[0u16, 32896, 32767, 65535, 128]).to_vec(),
            ColorFormat::L_U8,
            vec![0, 128, 127, 255, 0],
        ),
        (
            ColorFormat::L_F32,
            bytemuck::cast_slice(&[0.5f32, 0.2, 1.0, -1.0, 2.0]).to_vec(),
            ColorFormat::L_U8,
            vec![128, 51, 255, 0, 255],
        ),
        (
            ColorFormat::L_U8,
            vec![0, 51, 255, 1, 2],
            ColorFormat::RGBA_F32,
            bytemuck::cast_slice(&[0, 51, 255, 1, 2].map(|v: u8| {
                [
                    f32::from(v) / 255.0,
                    f32::from(v) / 255.0,
                    f32::from(v) / 255.0,
                    1.0,
                ]
            }))
            .to_vec(),
        ),
    ];
    for (from, bytes, to, expected) in cases {
        let image = Image::new_with_data(ImageDesc::new(5, 1, from), bytes).unwrap();
        let converted = image.convert_to(to);
        assert_eq!(converted.desc(), ImageDesc::new(5, 1, to));
        assert_eq!(converted.bytes(), expected, "{from} -> {to}");
    }
}

/// A conversion to the image's own format is a full copy, not a black image.
#[test]
fn converting_to_the_same_format_copies() {
    for format in ALL_FORMATS {
        let bytes: Vec<u8> = (0..3 * 2 * format.byte_count())
            .map(|i| (i * 37 % 251) as u8)
            .collect();
        let image = Image::new_with_data(ImageDesc::new(3, 2, format), bytes.clone()).unwrap();
        assert_eq!(image.convert_to(format).bytes(), bytes, "{format}");
        assert_eq!(image.convert(format).bytes(), bytes, "{format}");
    }
}
