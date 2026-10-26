//! The file cases: the standard frame saved to and read from each file format, in every color
//! format the file stores.

use std::hint::black_box;
use std::io::{BufReader, BufWriter, Cursor, Write};
use std::path::Path;
use std::time::Duration;

use common::bench::{Kind, Scale};
use common::{Size2us, TempDir, fs};
use criterion::{BenchmarkId, Criterion};

use crate::bench::fixture::{self, Backend};
use crate::bench::scene::{self, FRAME};
use crate::codec::raster_format::RasterFormat;
use crate::color::color_format::ColorFormat;
use crate::error::CodecError;
use crate::interleaved::interleaved_image::InterleavedImage;

/// `integration/read_file/cpu/<file>/<format>` and `integration/save_file/cpu/<file>/<format>`:
/// the standard frame decoded from a file and encoded into one, as a caller does through
/// `common::fs`, in every format the file stores. The file stays in the page cache, so a read times the decode, not the disk. Per pixel,
/// in the frame's uncompressed bytes.
pub(crate) fn integration(c: &mut Criterion) {
    let dir = TempDir::new("imaginarium-bench-file");
    let pixels = FRAME.pixel_count();

    let mut group = Kind::Integration.group(c, "save_file", Scale::Millis);
    for (file, format) in cases() {
        scale(file).apply(&mut group);
        group.throughput(fixture::throughput(pixels, format.byte_len()));
        let path = dir.join(format!("save.{}", extension(file)));
        let mut source = None;
        group.bench_function(
            BenchmarkId::new(
                Backend::Cpu.case(extension(file)),
                fixture::format_id(format),
            ),
            |b| {
                let frame = source.get_or_insert_with(|| scene::sky(format, FRAME, 1));
                b.iter(|| save(black_box(&*frame), &path, file));
            },
        );
    }
    group.finish();

    let mut group = Kind::Integration.group(c, "read_file", Scale::Millis);
    for (file, format) in cases() {
        scale(file).apply(&mut group);
        group.throughput(fixture::throughput(pixels, format.byte_len()));
        let mut path = None;
        group.bench_function(
            BenchmarkId::new(
                Backend::Cpu.case(extension(file)),
                fixture::format_id(format),
            ),
            |b| {
                let path = path.get_or_insert_with(|| {
                    let name = format!("read_{}.{}", fixture::format_id(format), extension(file));
                    let path = dir.join(name);
                    save(&scene::sky(format, FRAME, 1), &path, file);
                    path
                });
                b.iter_with_large_drop(|| read(black_box(&*path), file));
            },
        );
    }
    group.finish();
}

/// `image` encoded as `file` into a new file at `path`.
fn save(image: &InterleavedImage, path: &Path, file: RasterFormat) {
    let mut out = BufWriter::new(fs::create(path).unwrap());
    file.encode(image, &mut out).unwrap();
    out.flush().unwrap();
}

/// The `file` image at `path`.
fn read(path: &Path, file: RasterFormat) -> InterleavedImage {
    file.decode(BufReader::new(fs::open(path).unwrap()), u64::MAX)
        .unwrap()
}

/// Every file format with each color format it stores, found by encoding one pixel.
fn cases() -> Vec<(RasterFormat, ColorFormat)> {
    let pairs = RasterFormat::ALL
        .into_iter()
        .flat_map(|file| ColorFormat::ALL.map(|format| (file, format)));
    pairs
        .filter(|&(file, format)| {
            match file.encode(
                &scene::sky(format, Size2us::new(1, 1), 0),
                Cursor::new(Vec::new()),
            ) {
                Ok(()) => true,
                Err(CodecError::Unstorable { .. }) => false,
                Err(error) => panic!("probing {file:?} for {format}: {error}"),
            }
        })
        .collect()
}

const fn extension(file: RasterFormat) -> &'static str {
    file.extensions()[0]
}

/// The sampling of a file format's cases, from the slowest iteration of its saves and reads as
/// measured on a Ryzen 7 6800U: PNG 468 ms (RGBA u16), JPEG 336 ms (RGB u8), TIFF 201 ms.
const fn scale(file: RasterFormat) -> Scale {
    match file {
        RasterFormat::Png => Scale::Slow {
            iteration: Duration::from_millis(470),
        },
        RasterFormat::Jpeg => Scale::Slow {
            iteration: Duration::from_millis(340),
        },
        RasterFormat::Tiff => Scale::Millis,
    }
}
