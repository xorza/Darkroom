use std::io::Cursor;

use crate::color::color_format::ColorFormat;
use crate::error::Error;
use crate::image::InterleavedImage;
use crate::image::image_desc::InterleavedDesc;
use crate::image::raster_format::RasterFormat;
use crate::internals::{create_test_image, lena};
use common::{Buffer2, Size2us, par};
use rayon::iter::ParallelIterator;

#[test]
fn lena_reads_as_packed_rgba() {
    let img = lena(ColorFormat::RGBA_U8);
    assert_eq!(
        img.desc(),
        InterleavedDesc::new(Size2us::new(895, 551), ColorFormat::RGBA_U8)
    );
    assert_eq!(img.desc().row_byte_len(), 895 * 4);
    assert_eq!(img.bytes().len(), 895 * 4 * 551);
}

/// `image` encoded as `format` and decoded back.
fn round_trip(image: &InterleavedImage, format: RasterFormat) -> Result<InterleavedImage, Error> {
    let mut encoded = Cursor::new(Vec::new());
    image.encode(&mut encoded, format)?;
    encoded.set_position(0);
    InterleavedImage::decode(encoded, format)
}

/// TIFF stores all nine formats; each one reads back byte for byte.
#[test]
fn every_format_round_trips_through_tiff() {
    for format in ColorFormat::ALL {
        let image = create_test_image(format, Size2us::new(17, 5), 3);
        let reloaded = round_trip(&image, RasterFormat::Tiff).unwrap();
        assert_eq!(reloaded.desc(), image.desc(), "{format}");
        assert_eq!(reloaded.bytes(), image.bytes(), "{format}");
    }
}

/// PNG stores the integer formats and reads them back byte for byte; it has no float.
#[test]
fn integer_formats_round_trip_through_png() {
    for format in ColorFormat::ALL {
        let image = create_test_image(format, Size2us::new(17, 5), 3);
        let result = round_trip(&image, RasterFormat::Png);
        if format.sample_type.is_float() {
            assert!(
                matches!(result, Err(Error::UnsupportedFormat(message)) if message == format!("PNG cannot store {format}")),
                "{format}"
            );
            continue;
        }
        let reloaded = result.unwrap();
        assert_eq!(reloaded.desc(), image.desc(), "{format}");
        assert_eq!(reloaded.bytes(), image.bytes(), "{format}");
    }
}

/// JPEG takes 8-bit mono and RGB only.
#[test]
fn jpeg_refuses_what_it_cannot_store() {
    for format in ColorFormat::ALL {
        let image = create_test_image(format, Size2us::new(8, 8), 0);
        let result = image.encode(Cursor::new(Vec::new()), RasterFormat::Jpeg);
        if format == ColorFormat::MONO_U8 || format == ColorFormat::RGB_U8 {
            result.unwrap();
        } else {
            assert!(
                matches!(result, Err(Error::UnsupportedFormat(message)) if message == format!("JPEG cannot store {format}")),
                "{format}"
            );
        }
    }
}

/// A mono-with-alpha PNG has no format of its own; it reads as RGBA with its alpha.
#[test]
fn mono_alpha_png_widens_to_rgba() {
    let mut png = Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut png,
        &[10, 200, 30, 40],
        2,
        1,
        image::ColorType::La8,
        image::ImageFormat::Png,
    )
    .unwrap();
    png.set_position(0);
    let reloaded = InterleavedImage::decode(png, RasterFormat::Png).unwrap();
    assert_eq!(
        reloaded.desc(),
        InterleavedDesc::new(Size2us::new(2, 1), ColorFormat::RGBA_U8)
    );
    assert_eq!(reloaded.bytes(), &[10, 10, 10, 200, 30, 30, 30, 40]);
}

#[test]
fn construction_checks_dimensions_and_length() {
    assert!(matches!(
        InterleavedImage::new_with_data(InterleavedDesc::new(Size2us::new(1, 1), ColorFormat::MONO_U16), vec![0; 3]),
        Err(Error::SizeMismatch(message)) if message == "bytes length 3 does not match expected size 2"
    ));
    assert!(matches!(
        InterleavedImage::new_black(InterleavedDesc::new(Size2us::new(0, 4), ColorFormat::MONO_U8)),
        Err(Error::SizeMismatch(message)) if message == "image dimensions must be non-zero, got 0x4"
    ));
    assert!(matches!(
        InterleavedImage::new_black(InterleavedDesc::new(
            Size2us::new(usize::MAX / 2, 3),
            ColorFormat::RGBA_F32
        )),
        Err(Error::SizeMismatch(_))
    ));
}

/// A `u8` format keeps the caller's allocation both ways; `u16` and `f32` copy into storage
/// aligned for their type, which a `Vec<u8>` does not promise.
#[test]
fn byte_vectors_are_reused_where_the_layout_allows() {
    for format in ColorFormat::ALL {
        let desc = InterleavedDesc::new(Size2us::new(6, 2), format);
        let bytes: Vec<u8> = (0..desc.byte_len()).map(|i| (i * 7 % 251) as u8).collect();
        let given = bytes.clone();
        let pointer = given.as_ptr();
        let image = InterleavedImage::new_with_data(desc, given).unwrap();
        assert_eq!(image.bytes(), bytes, "{format}");
        assert_eq!(
            image.bytes().as_ptr() as usize % format.sample_type.byte_len(),
            0,
            "{format}: storage is aligned for its sample type"
        );
        assert_eq!(
            image.bytes().as_ptr() == pointer,
            format.sample_type.byte_len() == 1,
            "{format}"
        );

        let pointer = image.bytes().as_ptr();
        let back = image.into_bytes();
        assert_eq!(back, bytes, "{format}");
        assert_eq!(
            back.as_ptr() == pointer,
            format.sample_type.byte_len() == 1,
            "{format}"
        );
    }
}

/// The same format is a move-through: the very same allocation comes back.
#[test]
fn convert_to_the_same_format_keeps_the_allocation() {
    let image = create_test_image(ColorFormat::RGBA_U16, Size2us::new(3, 2), 0);
    let pointer = image.bytes().as_ptr();
    assert_eq!(
        image.convert(ColorFormat::RGBA_U16).bytes().as_ptr(),
        pointer
    );
}

/// RGB 2×1: planes R = [1, 4], G = [2, 5], B = [3, 6] ⟷ interleaved [1, 2, 3, 4, 5, 6].
#[test]
fn planes_interleave_into_the_format_they_spell() {
    let planes = [
        Buffer2::new((2, 1), vec![1u8, 4]),
        Buffer2::new((2, 1), vec![2u8, 5]),
        Buffer2::new((2, 1), vec![3u8, 6]),
    ];
    let image = InterleavedImage::from(planes.each_ref());
    assert_eq!(
        image.desc(),
        InterleavedDesc::new(Size2us::new(2, 1), ColorFormat::RGB_U8)
    );
    assert_eq!(image.bytes(), &[1, 2, 3, 4, 5, 6]);

    let back: [Buffer2<u8>; 3] = (&image).try_into().unwrap();
    assert_eq!(back, planes);
    let taken: [Buffer2<u8>; 3] = image.try_into().unwrap();
    assert_eq!(taken, planes);

    let mono = InterleavedImage::from([&Buffer2::new((3, 1), vec![10u16, 20, 30])]);
    assert_eq!(mono.desc().color_format, ColorFormat::MONO_U16);
    assert_eq!(mono.bytes(), bytemuck::cast_slice::<u16, u8>(&[10, 20, 30]));
    // A mono image taken is its one plane, in the same allocation.
    let pointer = mono.bytes().as_ptr();
    let [plane]: [Buffer2<u16>; 1] = mono.try_into().unwrap();
    assert_eq!(plane.pixels(), &[10, 20, 30]);
    assert_eq!(plane.pixels().as_ptr().cast::<u8>(), pointer);
}

/// Interleaving planes is parallel work, so a parallel loop's body that converts them panics, as
/// `common::par` refuses nested work; at top level the same conversion returns the image.
#[test]
#[should_panic(expected = "a parallel loop's body starts no parallel work")]
fn planes_converted_in_a_parallel_body_panic() {
    let plane = Buffer2::new((2, 1), vec![1.0f32, 2.0]);
    let convert = || {
        let image = InterleavedImage::from([&plane, &plane, &plane]);
        assert_eq!(
            image.desc(),
            InterleavedDesc::new(Size2us::new(2, 1), ColorFormat::RGB_F32)
        );
    };
    convert();
    par::into_iter(0..2).for_each(|_| convert());
}

#[test]
fn deinterleaving_into_another_format_is_refused() {
    let image = InterleavedImage::new_black(InterleavedDesc::new(
        Size2us::new(2, 2),
        ColorFormat::RGB_U8,
    ))
    .unwrap();
    let result: Result<[Buffer2<f32>; 1], Error> = (&image).try_into();
    assert!(matches!(
        result,
        Err(Error::InvalidColorFormat(message)) if message == "cannot deinterleave a RGB u8 image into 1 f32 planes"
    ));
    let result: Result<[Buffer2<f32>; 1], Error> = image.try_into();
    assert!(matches!(
        result,
        Err(Error::InvalidColorFormat(message)) if message == "cannot deinterleave a RGB u8 image into 1 f32 planes"
    ));
}

#[test]
#[should_panic(expected = "an image needs at least one pixel")]
fn empty_planes_are_refused() {
    let empty: Buffer2<f32> = Buffer2::new((0, 3), Vec::new());
    drop(InterleavedImage::from([&empty]));
}
