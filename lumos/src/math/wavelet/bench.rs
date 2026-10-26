//! One à trous smoothing step on a frame, at the hole spacings of the first four layers, and its
//! row kernel on each tier.

use std::hint::black_box;

use common::Buffer2;
use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture;
use crate::math::wavelet::{SmoothRow, atrous_smooth};
use simd::IsaTier;

const SIDE: usize = 2_048;
const STEPS: [usize; 4] = [1, 2, 4, 8];

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "math/wavelet/frame", Scale::Millis);
    group.throughput(Throughput::Elements((SIDE * SIDE) as u64));
    for step in STEPS {
        group.bench_function(
            BenchmarkId::new(format!("atrous_smooth/step_{step}"), SIDE),
            |b| {
                let image = Buffer2::new((SIDE, SIDE), fixture::sky(SIDE * SIDE, 6));
                let mut smooth = Buffer2::new_default((SIDE, SIDE));
                b.iter(|| atrous_smooth(black_box(&image), &mut smooth, step));
            },
        );
    }
    group.finish();

    let mut group = Kind::Atomic.group(c, "math/wavelet/row", Scale::Micros);
    group.throughput(Throughput::Elements(SIDE as u64));
    let image = Buffer2::new((SIDE, SIDE), fixture::sky(SIDE * SIDE, 6));
    let (mut col, mut out) = (vec![0.0f32; SIDE], vec![0.0f32; SIDE]);
    for step in STEPS {
        let y = SIDE / 2;
        let rows = [y - 2 * step, y - step, y, y + step, y + 2 * step].map(|row| image.row(row));
        for tier in IsaTier::supported() {
            group.bench_function(
                BenchmarkId::new(format!("smooth_row/step_{step}/{}", tier.id()), SIDE),
                |b| {
                    b.iter(|| {
                        tier.run(SmoothRow {
                            rows: black_box(rows),
                            col: &mut col,
                            out: &mut out,
                            step,
                        });
                    });
                },
            );
        }
    }
    group.finish();
}
