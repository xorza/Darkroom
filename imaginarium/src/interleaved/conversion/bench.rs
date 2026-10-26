//! The conversion cases, over every ordered pair of distinct formats: each row kernel on one row,
//! and `InterleavedImage::convert_to` on the standard frame.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion};

use common::Size2us;
use common::bench::{Kind, Scale};

use crate::bench::fixture::{self, Backend, Variant};
use crate::bench::scene::{self, FRAME};
use crate::color::color_format::ColorFormat;
use crate::interleaved::conversion::scalar;
use crate::interleaved::conversion::vector_conversion::VectorConversion;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;

/// `atomic/convert/cpu/<variant>/<from>/<to>`: one row of the standard frame through the scalar
/// reference and, for a pair with a vector kernel, that kernel on each tier this CPU has. Per
/// pixel, reading the source row.
pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "convert", Scale::Micros);
    for from in ColorFormat::ALL {
        let mut source = None;
        for to in pairs_from(from) {
            let mut row = InterleavedImage::new_zeroed(InterleavedDesc::new(
                Size2us::new(FRAME.width, 1),
                to,
            ));
            group.throughput(fixture::throughput(FRAME.width, from.byte_len()));
            for variant in Variant::all(VectorConversion::of(from, to)) {
                let id = BenchmarkId::new(Backend::Cpu.case(variant.id()), pair_id(from, to));
                group.bench_function(id, |b| {
                    let src = source
                        .get_or_insert_with(|| scene::sky(from, Size2us::new(FRAME.width, 1), 1))
                        .bytes();
                    let dst = row.bytes_mut();
                    if let Variant::Kernel { kernel, tier } = variant {
                        b.iter(|| {
                            kernel.convert_row(
                                tier,
                                black_box(src),
                                black_box(&mut *dst),
                                FRAME.width,
                            );
                        });
                    } else {
                        let convert = scalar::row_converter(from, to);
                        b.iter(|| convert(black_box(src), black_box(&mut *dst), FRAME.width));
                    }
                });
            }
        }
    }
    group.finish();
}

/// `integration/convert/cpu/<from>/<to>`: `InterleavedImage::convert_to` on the standard frame, the
/// allocation of the result included. Per pixel, reading the source frame.
pub(crate) fn integration(c: &mut Criterion) {
    let mut group = Kind::Integration.group(c, "convert", Scale::Millis);
    for from in ColorFormat::ALL {
        let mut source = None;
        for to in pairs_from(from) {
            group.throughput(fixture::throughput(FRAME.pixel_count(), from.byte_len()));
            group.bench_function(
                BenchmarkId::new(Backend::Cpu.id(), pair_id(from, to)),
                |b| {
                    let source = source.get_or_insert_with(|| scene::sky(from, FRAME, 1));
                    b.iter_with_large_drop(|| black_box(&*source).convert_to(to));
                },
            );
        }
    }
    group.finish();
}

/// Every format but `from`.
fn pairs_from(from: ColorFormat) -> impl Iterator<Item = ColorFormat> {
    ColorFormat::ALL.into_iter().filter(move |&to| to != from)
}

fn pair_id(from: ColorFormat, to: ColorFormat) -> String {
    format!("{}/{}", fixture::format_id(from), fixture::format_id(to))
}
