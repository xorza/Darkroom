use std::io::Cursor;

use common::Size2us;
use tiff::encoder::{TiffEncoder, colortype};

use crate::codec::raster_format::RasterFormat;
use crate::color::color_format::ColorFormat;
use crate::error::{CodecError, TiffSampleType};
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::{create_test_image, lena};

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
fn round_trip(
    image: &InterleavedImage,
    format: RasterFormat,
) -> Result<InterleavedImage, CodecError> {
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
                matches!(result, Err(CodecError::Unstorable { format: RasterFormat::Png, color_format }) if color_format == format),
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
                matches!(result, Err(CodecError::Unstorable { format: RasterFormat::Jpeg, color_format }) if color_format == format),
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

/// A TIFF of a color type or a sample type no format holds is refused with the one it states: a
/// CMYK file, and a gray file of `u32` samples.
#[test]
fn a_tiff_no_format_holds_is_refused() {
    fn tiff(write: impl FnOnce(&mut TiffEncoder<&mut Cursor<Vec<u8>>>)) -> CodecError {
        let mut file = Cursor::new(Vec::new());
        write(&mut TiffEncoder::new(&mut file).unwrap());
        file.set_position(0);
        InterleavedImage::decode(file, RasterFormat::Tiff).unwrap_err()
    }
    let cmyk = tiff(|encoder| {
        encoder
            .write_image::<colortype::CMYK8>(1, 1, &[0, 0, 0, 0])
            .unwrap();
    });
    assert!(
        matches!(cmyk, CodecError::TiffColor(tiff::ColorType::CMYK(8))),
        "{cmyk}"
    );
    let gray32 = tiff(|encoder| {
        encoder
            .write_image::<colortype::Gray32>(1, 1, &[7])
            .unwrap();
    });
    assert!(
        matches!(gray32, CodecError::TiffSamples(TiffSampleType::U32)),
        "{gray32}"
    );
}
