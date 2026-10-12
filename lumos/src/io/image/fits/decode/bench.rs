//! The FITS decode of a frame from a file the page cache holds: the conversion to normalized
//! samples, not the disk.

use std::path::PathBuf;

use common::TempDir;
use criterion::{BenchmarkId, Criterion, Throughput};
use fits_well::header::Header;
use fits_well::image::Image;

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::internals::fits::write_fits;
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_pattern::CfaPattern;
use crate::io::image::fits::decode::{load_cfa_fits, load_linear_fits};
use crate::io::image::load_context::LoadContext;
use crate::math::size2us::Size2us;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = fixture::group(c, "kernel/io/fits/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // A mono camera's 16-bit frame, a calibrated or stacked frame in 32-bit float, and a colour
    // camera's 16-bit mosaic, which loads as a CFA image.
    group.bench_function(BenchmarkId::new("decode/u16", field.name()), |b| {
        let file = FitsFile::write(Format::U16, field, FRAME);
        b.iter_with_large_drop(|| load_linear_fits(&file.path, &LoadContext::default()).unwrap());
    });
    group.bench_function(BenchmarkId::new("decode/f32", field.name()), |b| {
        let file = FitsFile::write(Format::F32, field, FRAME);
        b.iter_with_large_drop(|| load_linear_fits(&file.path, &LoadContext::default()).unwrap());
    });
    group.bench_function(BenchmarkId::new("decode/cfa_u16", field.name()), |b| {
        let file = FitsFile::write(Format::CfaU16, field, FRAME);
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
    fn write(format: Format, field: Field, size: Size2us) -> Self {
        let dir = TempDir::new("lumos-bench-fits");
        let path = dir.join("frame.fits");
        let shape = vec![size.width, size.height];
        match format {
            Format::U16 => {
                let frame = field.render(size);
                let samples = quantize(frame.image.channel(0).pixels());
                write_fits(&path, &Image::from_u16(shape, &samples).unwrap(), None);
            }
            Format::F32 => {
                let frame = field.render(size);
                let samples = frame.image.channel(0).pixels().to_vec();
                write_fits(&path, &Image::new(shape, samples).unwrap(), None);
            }
            Format::CfaU16 => {
                let mosaic = field.mosaic(size, CfaType::Bayer(CfaPattern::Rggb));
                let samples = quantize(mosaic.data.pixels());
                let mut header = Header::new();
                header.set("BAYERPAT", "RGGB").unwrap();
                write_fits(
                    &path,
                    &Image::from_u16(shape, &samples).unwrap(),
                    Some(&header),
                );
            }
        }
        Self { path, _dir: dir }
    }
}

/// `samples` in `[0, 1]` to the 16-bit full scale, rounded.
#[expect(clippy::cast_sign_loss, reason = "the samples are in [0, 1]")]
fn quantize(samples: &[f32]) -> Vec<u16> {
    samples
        .iter()
        .map(|&sample| (sample * f32::from(u16::MAX)).round() as u16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each file loads back as the field, to within half the 16-bit step `0.5 / 65535` for the
    /// integer formats and exactly for the float one.
    #[test]
    fn each_file_loads_back_as_its_field() {
        let size = Size2us::new(60, 40);
        let field = Field::Typical;
        let render = field.render(size);
        let render = render.image.channel(0).pixels();
        let mosaic = field.mosaic(size, CfaType::Bayer(CfaPattern::Rggb));
        let within_half_step = |loaded: &[f32], expected: &[f32]| {
            assert_eq!(loaded.len(), expected.len());
            for (&loaded, &expected) in loaded.iter().zip(expected) {
                let error = (f64::from(loaded) - f64::from(expected)).abs();
                assert!(error <= 0.5 / 65_535.0 + 1e-7, "{loaded} for {expected}");
            }
        };

        let u16 = FitsFile::write(Format::U16, field, size);
        let loaded = load_linear_fits(&u16.path, &LoadContext::default()).unwrap();
        within_half_step(loaded.channel(0).pixels(), render);

        let f32 = FitsFile::write(Format::F32, field, size);
        let loaded = load_linear_fits(&f32.path, &LoadContext::default()).unwrap();
        assert_eq!(loaded.channel(0).pixels(), render);

        let cfa = FitsFile::write(Format::CfaU16, field, size);
        let loaded = load_cfa_fits(&cfa.path, &LoadContext::default()).unwrap();
        assert_eq!(loaded.cfa_type, CfaType::Bayer(CfaPattern::Rggb));
        within_half_step(loaded.data.pixels(), mosaic.data.pixels());
    }
}
