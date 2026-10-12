//! One à trous smoothing step on a frame, at the hole spacings of the first four layers.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};
use imaginarium::Buffer2;

use crate::bench::fixture::{self, Scale};
use crate::math::wavelet::atrous_smooth;

const SIDE: usize = 2_048;
const STEPS: [usize; 4] = [1, 2, 4, 8];

pub(crate) fn benches(c: &mut Criterion) {
    let mut group = fixture::group(c, "kernel/math/wavelet/frame", Scale::Frame);
    group.throughput(Throughput::Elements((SIDE * SIDE) as u64));
    for step in STEPS {
        group.bench_function(
            BenchmarkId::new(format!("atrous_smooth/step_{step}"), SIDE),
            |b| {
                let image = Buffer2::new(SIDE, SIDE, fixture::sky(SIDE * SIDE, 6));
                let mut smooth = Buffer2::new_default(SIDE, SIDE);
                b.iter(|| atrous_smooth(black_box(&image), &mut smooth, step));
            },
        );
    }
    group.finish();
}
