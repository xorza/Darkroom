//! The conversion cases, over every ordered pair of distinct formats: each row kernel on one row,
//! and `Image::convert_to` on the standard frame.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion};

use crate::bench::fixture::{self, Kind, Scale, Variant};
use crate::bench::scene::{self, HEIGHT, WIDTH};
use crate::common::color_format::{ALL_FORMATS, ColorFormat};
use crate::image::Image;
use crate::image::conversion::{scalar, simd};
use crate::image::image_desc::ImageDesc;

/// `atomic/convert/<variant>/<from>/<to>`: one row of the standard frame through the scalar
/// reference and each tier's kernel. Per pixel, reading the source row.
pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "convert", Scale::Sample);
    for from in ALL_FORMATS {
        let mut source = None;
        for to in pairs_from(from) {
            let mut row = Image::new_black(ImageDesc::new(WIDTH, 1, to)).unwrap();
            group.throughput(fixture::throughput(WIDTH, from.byte_count()));
            for variant in Variant::all(|tier| simd::tier_converter(tier, from, to)) {
                let id = BenchmarkId::new(variant.name, pair_id(from, to));
                group.bench_function(id, |b| {
                    let src = source
                        .get_or_insert_with(|| scene::sky(from, WIDTH, 1, 1))
                        .bytes();
                    let dst = row.bytes_mut();
                    if let Some(kernel) = variant.kernel {
                        // SAFETY: a variant holds only kernels of tiers this CPU has.
                        b.iter(|| unsafe { kernel(black_box(src), black_box(&mut *dst), WIDTH) });
                    } else {
                        let convert = scalar::row_converter(from, to);
                        b.iter(|| convert(black_box(src), black_box(&mut *dst), WIDTH));
                    }
                });
            }
        }
    }
    group.finish();
}

/// `integration/convert/cpu/<from>/<to>`: `Image::convert_to` on the standard frame, the
/// allocation of the result included. Per pixel, reading the source frame.
pub(crate) fn integration(c: &mut Criterion) {
    let mut group = Kind::Integration.group(c, "convert", Scale::Frame);
    for from in ALL_FORMATS {
        let mut source = None;
        for to in pairs_from(from) {
            group.throughput(fixture::throughput(WIDTH * HEIGHT, from.byte_count()));
            group.bench_function(BenchmarkId::new("cpu", pair_id(from, to)), |b| {
                let source = source.get_or_insert_with(|| scene::sky(from, WIDTH, HEIGHT, 1));
                b.iter_with_large_drop(|| black_box(&*source).convert_to(to));
            });
        }
    }
    group.finish();
}

/// Every format but `from`.
fn pairs_from(from: ColorFormat) -> impl Iterator<Item = ColorFormat> {
    ALL_FORMATS.into_iter().filter(move |&to| to != from)
}

fn pair_id(from: ColorFormat, to: ColorFormat) -> String {
    format!("{}/{}", fixture::format_id(from), fixture::format_id(to))
}
