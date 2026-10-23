use std::cell::Cell;
use std::io;
use std::io::{BufRead, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use crate::common::buffer2::Buffer2;
use crate::common::color_format::{ALL_FORMATS, ColorFormat};
use crate::common::error::{AdmittedDecodeError, Error};
use crate::common::internals::{create_test_image, lena};
use crate::image::Image;
use crate::image::file_format::FileFormat;
use crate::image::image_desc::ImageDesc;

#[test]
fn lena_reads_as_packed_rgba() {
    let img = lena(ColorFormat::RGBA_U8);
    assert_eq!(img.desc(), ImageDesc::new(895, 551, ColorFormat::RGBA_U8));
    assert_eq!(img.desc().row_bytes(), 895 * 4);
    assert_eq!(img.bytes().len(), 895 * 4 * 551);
}

/// The extension picks the codec, case-insensitively; a missing or unknown one is refused
/// before any I/O, and a known one reads the file first, so a missing file is an I/O error
/// whatever its format.
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
        Err(Error::Io(_))
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
        assert_bytes_match_the_file(&image, FileFormat::Tiff, &path);
    }
}

/// `encode` writes what `save_file` puts in the file, and `decode` reads it back as `read_file`
/// does: the file calls are the bytes calls plus the file. An admitted decode reads the same, and
/// its admission sees the image it makes before it is made.
fn assert_bytes_match_the_file(image: &Image, format: FileFormat, path: &Path) {
    let mut encoded = Cursor::new(Vec::new());
    image.encode(&mut encoded, format).unwrap();
    let on_disk = std::fs::read(path).unwrap();
    assert_eq!(encoded.get_ref(), &on_disk, "{format:?} {}", image.desc());
    let decoded = Image::decode(Cursor::new(&on_disk), format).unwrap();
    assert_eq!(decoded.desc(), image.desc(), "{format:?}");
    assert_eq!(decoded.bytes(), image.bytes(), "{format:?}");
    let mut admitted = None;
    let decoded = Image::decode_admitted(Cursor::new(&on_disk), format, |desc| {
        admitted = Some(*desc);
        Ok::<_, ()>(())
    })
    .unwrap();
    assert_eq!(admitted, Some(image.desc()), "{format:?}");
    assert_eq!(decoded.bytes(), image.bytes(), "{format:?}");
}

/// A refusal ends the decode before any pixel is allocated or read. A TIFF whose directory
/// declares 60 000 × 60 000 RGB f32, 43 GB, over 64 × 48 pixels is refused with that
/// description; a PNG cut short after its header, which does not decode, is refused with the
/// description its header gives.
#[test]
fn a_refused_decode_reads_no_pixel() {
    let encoded = |color_format, format| {
        let mut encoded = Cursor::new(Vec::new());
        create_test_image(color_format, 64, 48, 1)
            .encode(&mut encoded, format)
            .unwrap();
        encoded.into_inner()
    };

    let mut tiff = encoded(ColorFormat::RGB_F32, FileFormat::Tiff);
    declare_tiff_size(&mut tiff, 60_000);
    let refused = Image::decode_admitted(Cursor::new(&tiff), FileFormat::Tiff, |desc| Err(*desc));
    assert!(matches!(
        refused,
        Err(AdmittedDecodeError::Refused(desc))
            if desc == ImageDesc::new(60_000, 60_000, ColorFormat::RGB_F32)
    ));

    let mut png = encoded(ColorFormat::L_U16, FileFormat::Png);
    png.truncate(png.len() / 2);
    assert!(
        Image::decode(Cursor::new(&png), FileFormat::Png).is_err(),
        "premise: the pixels are not all there"
    );
    let refused = Image::decode_admitted(Cursor::new(&png), FileFormat::Png, |desc| Err(*desc));
    assert!(matches!(
        refused,
        Err(AdmittedDecodeError::Refused(desc))
            if desc == ImageDesc::new(64, 48, ColorFormat::L_U16)
    ));
}

/// A TIFF of a sample type imaginarium stores no format of is refused from its header, by that
/// type's name, before any admission: 2 × 1 grey u32 and grey i16.
#[test]
fn a_tiff_of_another_sample_type_is_refused_from_its_header() {
    use tiff::encoder::TiffEncoder;
    use tiff::encoder::colortype::{Gray32, GrayI16};

    let mut u32_tiff = Cursor::new(Vec::new());
    TiffEncoder::new(&mut u32_tiff)
        .unwrap()
        .write_image::<Gray32>(2, 1, &[1, 2])
        .unwrap();
    let mut i16_tiff = Cursor::new(Vec::new());
    TiffEncoder::new(&mut i16_tiff)
        .unwrap()
        .write_image::<GrayI16>(2, 1, &[-1, 2])
        .unwrap();
    for (tiff, name) in [(u32_tiff, "u32"), (i16_tiff, "i16")] {
        let refused = Image::decode_admitted(
            Cursor::new(tiff.get_ref()),
            FileFormat::Tiff,
            |_| -> Result<(), ()> { panic!("a header of another sample type is not admitted") },
        );
        assert!(
            matches!(
                &refused,
                Err(AdmittedDecodeError::Decode(Error::UnsupportedFormat(message)))
                    if *message == format!("TIFF sample format not supported: {name}")
            ),
            "{refused:?}"
        );
    }
}

/// At its admission a decode has read its whole input only where its format says so: a 512 × 512
/// RGB u8 image of noise, 768 KiB raw, is read whole by JPEG and only to its header by
/// PNG and TIFF.
#[test]
fn only_a_format_that_reads_its_whole_input_has_read_it_at_admission() {
    let mut state = 1u32;
    let noise: Vec<u8> = (0..512 * 512 * 3)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect();
    let image = Image::new_with_data(ImageDesc::new(512, 512, ColorFormat::RGB_U8), noise).unwrap();
    for format in FileFormat::ALL {
        let mut encoded = Cursor::new(Vec::new());
        image.encode(&mut encoded, format).unwrap();
        let len = encoded.get_ref().len();
        let read = Cell::new(0);
        let input = Counted {
            inner: Cursor::new(encoded.get_ref()),
            read: &read,
        };
        let mut at_admission = 0;
        Image::decode_admitted(input, format, |_| {
            at_admission = read.get();
            Ok::<_, ()>(())
        })
        .unwrap();
        assert_eq!(
            at_admission == len,
            format.reads_whole_input(),
            "{format:?}: {at_admission} of {len} bytes read at admission"
        );
    }
}

/// A reader that counts the bytes read through it.
#[derive(Debug)]
struct Counted<'a, R> {
    inner: R,
    read: &'a Cell<usize>,
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buf)?;
        self.read.set(self.read.get() + count);
        Ok(count)
    }
}

impl<R: BufRead> BufRead for Counted<'_, R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.read.set(self.read.get() + amount);
        self.inner.consume(amount);
    }
}

impl<R: Seek> Seek for Counted<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Rewrite the first directory of the little-endian `tiff` to declare `side` × `side` pixels in
/// one strip, its tables consistent as a crafted header's are: the width, the height and the rows
/// a strip holds are `side`, and the strip tables keep their first entry.
fn declare_tiff_size(tiff: &mut [u8], side: u32) {
    let u16_at = |tiff: &[u8], at: usize| u16::from_le_bytes([tiff[at], tiff[at + 1]]);
    let u32_at = |tiff: &[u8], at: usize| u32::from_le_bytes(tiff[at..at + 4].try_into().unwrap());
    assert_eq!(&tiff[..2], b"II", "a little-endian TIFF");
    let directory = u32_at(tiff, 4) as usize;
    let mut patched = 0;
    for entry in 0..usize::from(u16_at(tiff, directory)) {
        let at = directory + 2 + entry * 12;
        let (tag, kind, count) = (u16_at(tiff, at), u16_at(tiff, at + 2), u32_at(tiff, at + 4));
        let value = match tag {
            // ImageWidth, ImageLength, RowsPerStrip.
            256 | 257 | 278 => side,
            // StripOffsets, StripByteCounts: the first strip's, held in the entry once it is one.
            273 | 279 if count > 1 => {
                assert_eq!(kind, 4, "a table of LONGs");
                u32_at(tiff, u32_at(tiff, at + 8) as usize)
            }
            273 | 279 => u32_at(tiff, at + 8),
            _ => continue,
        };
        tiff[at + 2..at + 4].copy_from_slice(&4u16.to_le_bytes());
        tiff[at + 4..at + 8].copy_from_slice(&1u32.to_le_bytes());
        tiff[at + 8..at + 12].copy_from_slice(&value.to_le_bytes());
        patched += 1;
    }
    assert_eq!(
        patched, 5,
        "the size, the rows of a strip and the strip tables"
    );
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
        assert_bytes_match_the_file(&image, FileFormat::Png, &path);
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
    let taken: [Buffer2<u8>; 3] = image.try_into().unwrap();
    assert_eq!(taken, planes);

    let grey = Image::from([&Buffer2::new(3, 1, vec![10u16, 20, 30])]);
    assert_eq!(grey.desc().color_format, ColorFormat::L_U16);
    assert_eq!(grey.bytes(), bytemuck::cast_slice::<u16, u8>(&[10, 20, 30]));
    // A grey image taken is its one plane, in the same allocation.
    let pointer = grey.bytes().as_ptr();
    let [plane]: [Buffer2<u16>; 1] = grey.try_into().unwrap();
    assert_eq!(plane.pixels(), &[10, 20, 30]);
    assert_eq!(plane.pixels().as_ptr().cast::<u8>(), pointer);
}

#[test]
fn deinterleaving_into_another_format_is_refused() {
    let image = Image::new_black(ImageDesc::new(2, 2, ColorFormat::RGB_U8)).unwrap();
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
    let empty: Buffer2<f32> = Buffer2::new(0, 3, Vec::new());
    drop(Image::from([&empty]));
}
