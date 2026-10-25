//! The preview cases: the standard frame down to darkroom's 256-pixel preview, from all nine
//! formats.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion};

use common::Size2us;
use common::bench::{Kind, Scale};

use crate::bench::fixture::{self, Backend};
use crate::bench::scene::{self, FRAME};
use crate::color::color_format::ALL_FORMATS;
use crate::ops::preview::Preview;

/// The preview's longer side, darkroom's preview texture.
const PREVIEW: usize = 256;

/// `integration/preview/cpu/<format>`: `Preview::to_rgba8` from the standard frame into a
/// 256 × 171 preview, the allocation of the result included. Per source pixel, reading the
/// frame.
pub(crate) fn integration(c: &mut Criterion) {
    let preview = Preview::new(Size2us::new(
        PREVIEW,
        (FRAME.height * PREVIEW + FRAME.width / 2) / FRAME.width,
    ));
    let mut group = Kind::Integration.group(c, "preview", Scale::Millis);
    for format in ALL_FORMATS {
        let pixels = FRAME.pixel_count();
        group.throughput(fixture::throughput(pixels, format.byte_count()));
        let mut source = None;
        group.bench_function(
            BenchmarkId::new(Backend::Cpu.id(), fixture::format_id(format)),
            |b| {
                let frame = source.get_or_insert_with(|| scene::sky(format, FRAME, 1));
                b.iter_with_large_drop(|| preview.to_rgba8(black_box(frame)));
            },
        );
    }
    group.finish();
}
