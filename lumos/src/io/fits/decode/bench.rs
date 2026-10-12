//! The FITS decode of a frame from a file the page cache holds: the conversion to normalized
//! samples, not the disk.

use std::path::PathBuf;

use common::TempDir;
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::files::{write_mono_f32, write_mosaic_u16};
use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::io::fits::decode::{load_cfa_fits, load_linear_fits};
use crate::io::load_context::LoadContext;

pub(crate) fn atomic(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = Kind::Atomic.group(c, "io/fits/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // A mono camera's 16-bit light, a calibrated or stacked frame in 32-bit float, and a colour
    // camera's 16-bit light, which loads as a CFA image.
    group.bench_function(BenchmarkId::new("decode/u16", field.name()), |b| {
        let file = FitsFile::write(Format::U16, field);
        b.iter_with_large_drop(|| load_linear_fits(&file.path, &LoadContext::default()).unwrap());
    });
    group.bench_function(BenchmarkId::new("decode/f32", field.name()), |b| {
        let file = FitsFile::write(Format::F32, field);
        b.iter_with_large_drop(|| load_linear_fits(&file.path, &LoadContext::default()).unwrap());
    });
    group.bench_function(BenchmarkId::new("decode/cfa_u16", field.name()), |b| {
        let file = FitsFile::write(Format::CfaU16, field);
        b.iter_with_large_drop(|| load_cfa_fits(&file.path, &LoadContext::default()).unwrap());
    });
}

/// How a frame is stored.
#[derive(Debug, Clone, Copy)]
enum Format {
    /// `BITPIX = 16` with `BZERO = 32768`, mono.
    U16,
    /// `BITPIX = -32`, normalized, mono.
    F32,
    /// `BITPIX = 16` with `BZERO = 32768` and `BAYERPAT = 'RGGB'`.
    CfaU16,
}

/// A field written to a FITS file in a directory of its own, removed with it.
#[derive(Debug)]
struct FitsFile {
    path: PathBuf,
    _dir: TempDir,
}

impl FitsFile {
    fn write(format: Format, field: Field) -> Self {
        let dir = TempDir::new("lumos-bench-fits");
        let path = dir.join("frame.fits");
        match format {
            Format::U16 => write_mosaic_u16(&path, &field.light(FRAME, CfaType::Mono)),
            Format::F32 => write_mono_f32(&path, &field.render(FRAME).image),
            Format::CfaU16 => {
                write_mosaic_u16(&path, &field.light(FRAME, CfaType::Bayer(CfaPattern::Rggb)));
            }
        }
        Self { path, _dir: dir }
    }
}
