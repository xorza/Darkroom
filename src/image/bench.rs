//! The file cases: the standard frame saved to and read from each file format, in every colour
//! format the file stores.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};

use crate::bench::fixture::{self, Kind, Scale, ScratchDir};
use crate::bench::scene::{self, HEIGHT, WIDTH};
use crate::common::color_format::{ALL_FORMATS, ColorFormat};
use crate::common::error::Error;
use crate::image::Image;
use crate::image::file_format::FileFormat;

/// `integration/read_file/<file>/<format>` and `integration/save_file/<file>/<format>`:
/// `Image::read_file` and `Image::save_file` on the standard frame, in every format the file
/// stores. The file stays in the page cache, so a read times the decode, not the disk. Per pixel,
/// in the frame's uncompressed bytes.
pub(crate) fn integration(c: &mut Criterion) {
    let dir = ScratchDir::new("file");
    let pixels = WIDTH * HEIGHT;

    let mut group = Kind::Integration.group(c, "save_file", Scale::Frame);
    for (file, format) in cases(&dir) {
        scale(file).apply(&mut group);
        group.throughput(fixture::throughput(pixels, format.byte_count()));
        let path = dir.file(&format!("save.{}", extension(file)));
        let mut source = None;
        group.bench_function(
            BenchmarkId::new(extension(file), fixture::format_id(format)),
            |b| {
                let frame = source.get_or_insert_with(|| scene::sky(format, WIDTH, HEIGHT, 1));
                b.iter(|| black_box(&*frame).save_file(&path).unwrap());
            },
        );
    }
    group.finish();

    let mut group = Kind::Integration.group(c, "read_file", Scale::Frame);
    for (file, format) in cases(&dir) {
        scale(file).apply(&mut group);
        group.throughput(fixture::throughput(pixels, format.byte_count()));
        let mut path = None;
        group.bench_function(
            BenchmarkId::new(extension(file), fixture::format_id(format)),
            |b| {
                let path = path.get_or_insert_with(|| {
                    let name = format!("read_{}.{}", fixture::format_id(format), extension(file));
                    let path = dir.file(&name);
                    scene::sky(format, WIDTH, HEIGHT, 1)
                        .save_file(&path)
                        .unwrap();
                    path
                });
                b.iter_with_large_drop(|| Image::read_file(black_box(&*path)).unwrap());
            },
        );
    }
    group.finish();
}

/// Every file format with each colour format it stores, found by saving one pixel.
fn cases(dir: &ScratchDir) -> Vec<(FileFormat, ColorFormat)> {
    let pairs = FileFormat::ALL
        .into_iter()
        .flat_map(|file| ALL_FORMATS.map(|format| (file, format)));
    pairs
        .filter(|&(file, format)| {
            let path = dir.file(&format!("probe.{}", extension(file)));
            match scene::sky(format, 1, 1, 0).save_file(path) {
                Ok(()) => true,
                Err(Error::UnsupportedFormat(_)) => false,
                Err(error) => panic!("probing {file:?} for {format}: {error}"),
            }
        })
        .collect()
}

const fn extension(file: FileFormat) -> &'static str {
    file.extensions()[0]
}

/// The sampling of a file format's cases, from the slowest iteration of its saves and reads as
/// measured on a Ryzen 7 6800U: PNG 468 ms (RGBA u16), JPEG 336 ms (RGB u8), TIFF 201 ms.
const fn scale(file: FileFormat) -> Scale {
    match file {
        FileFormat::Png => Scale::Set {
            iteration: Duration::from_millis(470),
        },
        FileFormat::Jpeg => Scale::Set {
            iteration: Duration::from_millis(340),
        },
        FileFormat::Tiff => Scale::Frame,
    }
}
