use crate::color::color_format::ColorFormat;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;
use crate::ops::preview::Preview;
use crate::planar::planar_image::PlanarImage;
use common::Size2us;

fn img(w: usize, h: usize, fmt: ColorFormat, bytes: Vec<u8>) -> InterleavedImage {
    InterleavedImage::new_with_data(InterleavedDesc::new(Size2us::new(w, h), fmt), bytes).unwrap()
}

/// A source's format, width, height and bytes, the target, and the preview's bytes.
type Row = (ColorFormat, usize, usize, Vec<u8>, Size2us, Vec<u8>);

/// `values` as the bytes of `T` samples.
fn bytes_of<T: bytemuck::Pod>(values: &[T]) -> Vec<u8> {
    bytemuck::cast_slice(values).to_vec()
}

/// Each row previews a `width` × `height` source of `format` at `target`, by hand:
///
/// - A same-size preview is a per-pixel conversion: `RGBA_U8` round-trips byte for byte, an RGB
///   pixel gains an opaque alpha, `u16` 65535 → 255 and 32768 → round(127.502) = 128, `f32`
///   1 → 255, 0.2 → 51.
/// - 2×2 → 1×1 averages all four pixels: (0 + 100 + 200 + 40) / 4 = 85, opaque alphas stay 255,
///   and mono broadcasts to RGB.
/// - 3 → 2 tiles the source by integer floor into {0} and {1, 2}: 0 and (60 + 120) / 2 = 90.
/// - A mean on a tie rounds to even: (1 + 2) / 2 = 1.5 → 2, (2 + 3) / 2 = 2.5 → 2, and in `u16`
///   (257 + 514) / 2 / 257 = 1.5 → 2.
/// - With alpha, each color weighs its pixels by alpha: opaque red beside transparent black is
///   red, `255·255 / 255`, at alpha 255 / 2 = 127.5 → 128, in `u8`, `u16` and `f32` alike. A
///   footprint of no alpha takes the plain mean: (10 + 30) / 2 = 20.
#[test]
fn a_preview_averages_each_footprint_as_computed_by_hand() {
    let rows: [Row; 12] = [
        (
            ColorFormat::RGBA_U8,
            2,
            2,
            (10..=160).step_by(10).collect(),
            Size2us::new(2, 2),
            (10..=160).step_by(10).collect(),
        ),
        (
            ColorFormat::RGB_U8,
            1,
            1,
            vec![12, 34, 56],
            Size2us::new(1, 1),
            vec![12, 34, 56, 255],
        ),
        (
            ColorFormat::RGBA_U16,
            1,
            1,
            bytes_of(&[65535u16, 32768, 0, 65535]),
            Size2us::new(1, 1),
            vec![255, 128, 0, 255],
        ),
        (
            ColorFormat::RGB_F32,
            1,
            1,
            bytes_of(&[1.0f32, 0.0, 0.2]),
            Size2us::new(1, 1),
            vec![255, 0, 51, 255],
        ),
        (
            ColorFormat::RGBA_U8,
            2,
            2,
            vec![
                0, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255, 40, 40, 40, 255,
            ],
            Size2us::new(1, 1),
            vec![85, 85, 85, 255],
        ),
        (
            ColorFormat::MONO_U8,
            2,
            2,
            vec![0, 100, 200, 40],
            Size2us::new(1, 1),
            vec![85, 85, 85, 255],
        ),
        (
            ColorFormat::MONO_U8,
            3,
            1,
            vec![0, 60, 120],
            Size2us::new(2, 1),
            vec![0, 0, 0, 255, 90, 90, 90, 255],
        ),
        (
            ColorFormat::MONO_U8,
            4,
            1,
            vec![1, 2, 2, 3],
            Size2us::new(2, 1),
            vec![2, 2, 2, 255, 2, 2, 2, 255],
        ),
        (
            ColorFormat::MONO_U16,
            2,
            1,
            bytes_of(&[257u16, 514]),
            Size2us::new(1, 1),
            vec![2, 2, 2, 255],
        ),
        (
            ColorFormat::RGBA_U8,
            2,
            1,
            vec![255, 0, 0, 255, 0, 0, 0, 0],
            Size2us::new(1, 1),
            vec![255, 0, 0, 128],
        ),
        (
            ColorFormat::RGBA_U16,
            2,
            1,
            bytes_of(&[65535u16, 0, 0, 65535, 0, 0, 0, 0]),
            Size2us::new(1, 1),
            vec![255, 0, 0, 128],
        ),
        (
            ColorFormat::RGBA_F32,
            2,
            1,
            bytes_of(&[1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
            Size2us::new(1, 1),
            vec![255, 0, 0, 128],
        ),
    ];
    for (format, width, height, bytes, target, expected) in rows {
        let preview = Preview::new(target).to_rgba8(&img(width, height, format, bytes));
        assert_eq!(preview.desc().size(), target, "{format} {width}×{height}");
        assert_eq!(
            preview.bytes(),
            expected,
            "{format} {width}×{height} → {target}"
        );
    }
    let clear = img(
        2,
        1,
        ColorFormat::RGBA_U8,
        vec![10, 20, 30, 0, 30, 40, 50, 0],
    );
    assert_eq!(
        Preview::new(Size2us::new(1, 1)).to_rgba8(&clear).bytes(),
        [20, 30, 40, 0]
    );
}

/// `f32` planes preview as their interleaved image does, byte for byte, mono and RGB, at a
/// downscale whose footprints are uneven.
#[test]
fn planes_preview_as_their_interleaved_image() {
    for format in [ColorFormat::MONO_F32, ColorFormat::RGB_F32] {
        let image = create_test_image(format, Size2us::new(17, 11), 3);
        let planes = PlanarImage::try_from(&image).unwrap();
        let preview = Preview::new(Size2us::new(8, 5));
        assert_eq!(
            preview.planes_to_rgba8(&planes).bytes(),
            preview.to_rgba8(&image).bytes(),
            "{format}"
        );
    }
}

/// A one-pixel footprint is the format conversion to `RGBA_U8`, for every format.
#[test]
fn test_same_size_is_the_conversion() {
    for format in ColorFormat::ALL {
        let src = create_test_image(format, Size2us::new(7, 3), 5);
        let out = Preview::new(Size2us::new(7, 3)).to_rgba8(&src);
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
    for format in ColorFormat::ALL {
        let src = create_test_image(format, Size2us::new(17, 11), 3);
        let out = Preview::new(Size2us::new(8, 5)).to_rgba8(&src);
        assert_eq!(out.desc().color_format(), ColorFormat::RGBA_U8);
        assert_eq!(out.desc().size().width, 8);
        assert_eq!(out.desc().size().height, 5);
        assert_eq!(out.bytes().len(), 8 * 5 * 4);
    }
}
