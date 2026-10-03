use std::path::Path;

use crate::common::buffer2::Buffer2;
use crate::common::color_format::{ALL_FORMATS, ColorFormat};
use crate::common::error::Error;
use crate::common::internals::{create_test_image, lena};
use crate::image::Image;
use crate::image::image_desc::ImageDesc;

#[test]
fn lena_reads_as_packed_rgba() {
    let img = lena(ColorFormat::RGBA_U8);
    assert_eq!(img.desc(), ImageDesc::new(895, 551, ColorFormat::RGBA_U8));
    assert_eq!(img.desc().row_bytes(), 895 * 4);
    assert_eq!(img.bytes().len(), 895 * 4 * 551);
}

/// The extension picks the codec, case-insensitively; a missing or unknown one is refused
/// before any I/O.
#[test]
fn read_file_dispatches_on_the_extension() {
    assert!(matches!(
        Image::read_file("/nonexistent/file.xyz"),
        Err(Error::InvalidExtension(path)) if path == Path::new("/nonexistent/file.xyz")
    ));
    assert!(matches!(
        Image::read_file("/nonexistent/file"),
        Err(Error::InvalidExtension(path)) if path == Path::new("/nonexistent/file")
    ));
    assert!(matches!(
        Image::read_file("/nonexistent/does_not_exist.PNG"),
        Err(Error::ImageCodec(_))
    ));
    assert!(matches!(
        Image::read_file("/nonexistent/does_not_exist.TIF"),
        Err(Error::Io(_))
    ));
}

/// TIFF stores all nine formats; each one reads back byte for byte.
#[test]
fn every_format_round_trips_through_tiff() {
    let dir = tempfile::tempdir().unwrap();
    for format in ALL_FORMATS {
        let image = create_test_image(format, 17, 5, 3);
        let path = dir.path().join(format!("{format}.tiff"));
        image.save_file(&path).unwrap();
        let reloaded = Image::read_file(&path).unwrap();
        assert_eq!(reloaded.desc(), image.desc(), "{format}");
        assert_eq!(reloaded.bytes(), image.bytes(), "{format}");
    }
}

/// PNG stores the integer formats and reads them back byte for byte; it has no float.
#[test]
fn integer_formats_round_trip_through_png() {
    let dir = tempfile::tempdir().unwrap();
    for format in ALL_FORMATS {
        let image = create_test_image(format, 17, 5, 3);
        let path = dir.path().join(format!("{format}.png"));
        if format.sample_type.is_float() {
            assert!(
                matches!(image.save_file(&path), Err(Error::UnsupportedFormat(message)) if message == format!("PNG cannot store {format}")),
                "{format}"
            );
            continue;
        }
        image.save_file(&path).unwrap();
        let reloaded = Image::read_file(&path).unwrap();
        assert_eq!(reloaded.desc(), image.desc(), "{format}");
        assert_eq!(reloaded.bytes(), image.bytes(), "{format}");
    }
}

/// JPEG takes 8-bit grey and RGB only.
#[test]
fn jpeg_refuses_what_it_cannot_store() {
    let dir = tempfile::tempdir().unwrap();
    for format in ALL_FORMATS {
        let image = create_test_image(format, 8, 8, 0);
        let result = image.save_file(dir.path().join(format!("{format}.jpg")));
        if format == ColorFormat::L_U8 || format == ColorFormat::RGB_U8 {
            result.unwrap();
        } else {
            assert!(
                matches!(result, Err(Error::UnsupportedFormat(message)) if message == format!("JPEG cannot store {format}")),
                "{format}"
            );
        }
    }
}

/// A grey-with-alpha PNG has no format of its own; it reads as RGBA with its alpha.
#[test]
fn grey_alpha_png_widens_to_rgba() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("la.png");
    image::save_buffer(&path, &[10, 200, 30, 40], 2, 1, image::ColorType::La8).unwrap();
    let reloaded = Image::read_file(&path).unwrap();
    assert_eq!(reloaded.desc(), ImageDesc::new(2, 1, ColorFormat::RGBA_U8));
    assert_eq!(reloaded.bytes(), &[10, 10, 10, 200, 30, 30, 30, 40]);
}

#[test]
fn construction_checks_dimensions_and_length() {
    assert!(matches!(
        Image::new_with_data(ImageDesc::new(1, 1, ColorFormat::L_U16), vec![0; 3]),
        Err(Error::SizeMismatch(message)) if message == "bytes length 3 does not match expected size 2"
    ));
    assert!(matches!(
        Image::new_black(ImageDesc::new(0, 4, ColorFormat::L_U8)),
        Err(Error::SizeMismatch(message)) if message == "image dimensions must be non-zero, got 0x4"
    ));
    assert!(matches!(
        Image::new_black(ImageDesc::new(usize::MAX / 2, 3, ColorFormat::RGBA_F32)),
        Err(Error::SizeMismatch(_))
    ));
}

/// A `u8` format keeps the caller's allocation both ways; `u16` and `f32` copy into storage
/// aligned for their type, which a `Vec<u8>` does not promise.
#[test]
fn byte_vectors_are_reused_where_the_layout_allows() {
    for format in ALL_FORMATS {
        let desc = ImageDesc::new(6, 2, format);
        let bytes: Vec<u8> = (0..desc.size_in_bytes())
            .map(|i| (i * 7 % 251) as u8)
            .collect();
        let given = bytes.clone();
        let pointer = given.as_ptr();
        let image = Image::new_with_data(desc, given).unwrap();
        assert_eq!(image.bytes(), bytes, "{format}");
        assert_eq!(
            image.bytes().as_ptr() as usize % format.sample_type.size(),
            0,
            "{format}: storage is aligned for its sample type"
        );
        assert_eq!(
            image.bytes().as_ptr() == pointer,
            format.sample_type.size() == 1,
            "{format}"
        );

        let pointer = image.bytes().as_ptr();
        let back = image.into_bytes();
        assert_eq!(back, bytes, "{format}");
        assert_eq!(
            back.as_ptr() == pointer,
            format.sample_type.size() == 1,
            "{format}"
        );
    }
}

/// The same format is a move-through: the very same allocation comes back.
#[test]
fn convert_to_the_same_format_keeps_the_allocation() {
    let image = create_test_image(ColorFormat::RGBA_U16, 3, 2, 0);
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
        Buffer2::new(2, 1, vec![1u8, 4]),
        Buffer2::new(2, 1, vec![2u8, 5]),
        Buffer2::new(2, 1, vec![3u8, 6]),
    ];
    let image = Image::from(planes.each_ref());
    assert_eq!(image.desc(), ImageDesc::new(2, 1, ColorFormat::RGB_U8));
    assert_eq!(image.bytes(), &[1, 2, 3, 4, 5, 6]);

    let back: [Buffer2<u8>; 3] = (&image).try_into().unwrap();
    assert_eq!(back, planes);

    let grey = Image::from([&Buffer2::new(3, 1, vec![10u16, 20, 30])]);
    assert_eq!(grey.desc().color_format, ColorFormat::L_U16);
    assert_eq!(grey.bytes(), bytemuck::cast_slice::<u16, u8>(&[10, 20, 30]));
}

#[test]
fn deinterleaving_into_another_format_is_refused() {
    let image = Image::new_black(ImageDesc::new(2, 2, ColorFormat::RGB_U8)).unwrap();
    let result: Result<[Buffer2<f32>; 1], Error> = (&image).try_into();
    assert!(matches!(
        result,
        Err(Error::InvalidColorFormat(message)) if message == "cannot deinterleave a RGB u8 image into 1 f32 planes"
    ));
}

#[test]
#[should_panic(expected = "an image needs at least one pixel")]
fn empty_planes_are_refused() {
    let empty: Buffer2<f32> = Buffer2::new(0, 3, Vec::new());
    drop(Image::from([&empty]));
}
