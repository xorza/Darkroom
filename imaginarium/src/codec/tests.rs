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
    format.encode(image, &mut encoded)?;
    encoded.set_position(0);
    format.decode(encoded, u64::MAX)
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
        let result = RasterFormat::Jpeg.encode(&image, Cursor::new(Vec::new()));
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
    let reloaded = RasterFormat::Png.decode(png, u64::MAX).unwrap();
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
        RasterFormat::Tiff.decode(file, u64::MAX).unwrap_err()
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

/// An uncompressed little-endian TIFF of one strip a plane, written tag by tag: `planes` holds one
/// strip for a chunky file and one per sample for a planar one, and `extra` is the
/// `ExtraSamples` value of a pixel with one more sample than its photometric names.
fn raw_tiff(
    size: [u32; 2],
    bits: &[u16],
    photometric: u16,
    planes: &[&[u8]],
    extra: Option<u16>,
) -> Cursor<Vec<u8>> {
    let samples = u16::try_from(bits.len()).unwrap();
    let planar = if planes.len() > 1 { 2 } else { 1 };
    let mut entries: Vec<(u16, u16, Vec<u32>)> = vec![
        (256, 4, vec![size[0]]),
        (257, 4, vec![size[1]]),
        (258, 3, bits.iter().map(|&bits| u32::from(bits)).collect()),
        (259, 3, vec![1]),
        (262, 3, vec![u32::from(photometric)]),
        (273, 4, vec![0; planes.len()]),
        (277, 3, vec![u32::from(samples)]),
        (278, 4, vec![size[1]]),
        (
            279,
            4,
            planes.iter().map(|plane| plane.len() as u32).collect(),
        ),
        (284, 3, vec![planar]),
    ];
    if let Some(extra) = extra {
        entries.push((338, 3, vec![u32::from(extra)]));
    }
    let ifd_len = 2 + 12 * entries.len() + 4;
    let mut data_at = 8 + ifd_len;
    // Values past four bytes go after the IFD, then the strips.
    let width_of = |kind: u16| if kind == 3 { 2 } else { 4 };
    let mut out_of_line = Vec::new();
    let mut offsets = Vec::new();
    for (_, kind, values) in &entries {
        let len = width_of(*kind) * values.len();
        offsets.push((len > 4).then(|| {
            let at = data_at + out_of_line.len();
            for &value in values {
                if *kind == 3 {
                    out_of_line.extend_from_slice(&u16::try_from(value).unwrap().to_le_bytes());
                } else {
                    out_of_line.extend_from_slice(&value.to_le_bytes());
                }
            }
            at
        }));
    }
    data_at += out_of_line.len();
    let mut strip_at = data_at;
    let strip_offsets: Vec<u32> = planes
        .iter()
        .map(|plane| {
            let at = strip_at;
            strip_at += plane.len();
            u32::try_from(at).unwrap()
        })
        .collect();
    entries[5].2 = strip_offsets;
    // The strip offsets changed after their place was taken: write them again where they went.
    if let Some(at) = offsets[5] {
        let at = at - (8 + ifd_len);
        for (k, &value) in entries[5].2.iter().enumerate() {
            out_of_line[at + 4 * k..at + 4 * k + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    let mut file = b"II*\0".to_vec();
    file.extend_from_slice(&8u32.to_le_bytes());
    file.extend_from_slice(&u16::try_from(entries.len()).unwrap().to_le_bytes());
    for ((tag, kind, values), offset) in entries.iter().zip(&offsets) {
        file.extend_from_slice(&tag.to_le_bytes());
        file.extend_from_slice(&kind.to_le_bytes());
        file.extend_from_slice(&u32::try_from(values.len()).unwrap().to_le_bytes());
        let mut field = [0u8; 4];
        match offset {
            Some(at) => field = u32::try_from(*at).unwrap().to_le_bytes(),
            None => {
                for (k, &value) in values.iter().enumerate() {
                    if *kind == 3 {
                        field[2 * k..2 * k + 2]
                            .copy_from_slice(&u16::try_from(value).unwrap().to_le_bytes());
                    } else {
                        field = value.to_le_bytes();
                    }
                }
            }
        }
        file.extend_from_slice(&field);
    }
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&out_of_line);
    for plane in planes {
        file.extend_from_slice(plane);
    }
    Cursor::new(file)
}

/// A TIFF whose samples are no format's is refused rather than read into one: a gray file of
/// 12-bit samples, and one of 1-bit samples ten to a row, by their width; a planar RGB file, whose
/// first plane alone the decoder reads, 2 samples of the 6 a 2 × 1 RGB image has. An RGB file
/// with a fourth sample of no stated meaning reads, the decoder leaving that sample out.
#[test]
fn a_tiff_of_samples_no_format_lays_out_is_refused() {
    let decode = |file| RasterFormat::Tiff.decode(file, u64::MAX).unwrap_err();
    let twelve = decode(raw_tiff([2, 1], &[12], 1, &[&[0x12, 0x34, 0x56]], None));
    assert!(matches!(twelve, CodecError::TiffBitDepth(12)), "{twelve}");
    let one = decode(raw_tiff([10, 1], &[1], 1, &[&[0xAA, 0x80]], None));
    assert!(matches!(one, CodecError::TiffBitDepth(1)), "{one}");

    let planar = decode(raw_tiff(
        [2, 1],
        &[8, 8, 8],
        2,
        &[&[1, 2], &[3, 4], &[5, 6]],
        None,
    ));
    assert!(
        matches!(planar, CodecError::TiffLayout { desc, decoded: 2 } if desc.sample_count() == 6),
        "{planar}"
    );
    let extra = RasterFormat::Tiff
        .decode(
            raw_tiff(
                [2, 1],
                &[8, 8, 8, 8],
                2,
                &[&[1, 2, 3, 4, 5, 6, 7, 8]],
                Some(0),
            ),
            u64::MAX,
        )
        .unwrap();
    assert_eq!(extra.desc().color_format(), ColorFormat::RGB_U8);
    assert_eq!(extra.bytes(), &[1, 2, 3, 5, 6, 7]);
}

/// A header stating more samples than the cap is refused before they are allocated, the cap one
/// byte short of them: a 4 × 2 RGB `u8` TIFF states 24 bytes and a 4 × 2 gray PNG 8; each reads
/// at the cap.
#[test]
fn a_header_past_the_cap_is_refused() {
    let tiff = || raw_tiff([4, 2], &[8, 8, 8], 2, &[&[7; 24]], None);
    let refused = RasterFormat::Tiff.decode(tiff(), 23).unwrap_err();
    assert!(
        matches!(
            refused,
            CodecError::Oversized {
                stated: 24,
                max_bytes: 23
            }
        ),
        "{refused}"
    );
    RasterFormat::Tiff.decode(tiff(), 24).unwrap();

    let gray = InterleavedImage::new_with_data(
        InterleavedDesc::new(Size2us::new(4, 2), ColorFormat::MONO_U8),
        vec![9; 8],
    )
    .unwrap();
    let mut png = Cursor::new(Vec::new());
    RasterFormat::Png.encode(&gray, &mut png).unwrap();
    let png = || Cursor::new(png.get_ref().clone());
    let refused = RasterFormat::Png.decode(png(), 7).unwrap_err();
    assert!(
        matches!(
            refused,
            CodecError::Oversized {
                stated: 8,
                max_bytes: 7
            }
        ),
        "{refused}"
    );
    RasterFormat::Png.decode(png(), 8).unwrap();
}
