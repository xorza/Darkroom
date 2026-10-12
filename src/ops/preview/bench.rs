//! The preview cases: the standard frame down to darkroom's 256-pixel preview, from all nine
//! formats.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion};

use crate::bench::fixture::{self, Backend, Kind, Scale};
use crate::bench::scene::{self, HEIGHT, WIDTH};
use crate::common::color_format::ALL_FORMATS;
use crate::ops::preview::Preview;

/// The preview's longer side, darkroom's preview texture.
const PREVIEW: usize = 256;

/// `integration/preview/cpu/<format>`: `Preview::to_rgba8` from the standard frame into a
/// 256 × 171 preview, the allocation of the result included. Per source pixel, reading the
/// frame.
pub(crate) fn integration(c: &mut Criterion) {
    let preview = Preview::new(PREVIEW, (HEIGHT * PREVIEW + WIDTH / 2) / WIDTH);
    let mut group = Kind::Integration.group(c, "preview", Scale::Frame);
    for format in ALL_FORMATS {
        let pixels = WIDTH * HEIGHT;
        group.throughput(fixture::throughput(pixels, format.byte_count()));
        let mut source = None;
        group.bench_function(
            BenchmarkId::new(Backend::Cpu.id(), fixture::format_id(format)),
            |b| {
                let frame = source.get_or_insert_with(|| scene::sky(format, WIDTH, HEIGHT, 1));
                b.iter_with_large_drop(|| preview.to_rgba8(black_box(frame)));
            },
        );
    }
    group.finish();
}
