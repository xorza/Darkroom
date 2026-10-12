//! The scalar table read drizzle takes per tap, on each order's table.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

use crate::bench::fixture::{self, Scale};
use crate::math::lanczos::lanczos_lut::{LANCZOS_LUT_RESOLUTION, LanczosOrder};

const READS: usize = 4_096;

pub(crate) fn benches(c: &mut Criterion) {
    let mut group = fixture::group(c, "kernel/math/lanczos", Scale::Sample);
    group.throughput(Throughput::Elements(READS as u64));
    for order in [LanczosOrder::Two, LanczosOrder::Three, LanczosOrder::Four] {
        group.bench_function(
            BenchmarkId::new(format!("lut_at/a{}", order.a()), READS),
            |b| {
                let lut = order.lut();
                let mut rng = ChaCha8Rng::seed_from_u64(7);
                let reach = (order.a() * LANCZOS_LUT_RESOLUTION) as f32;
                let distances: Vec<f32> = (0..READS).map(|_| rng.random::<f32>() * reach).collect();
                b.iter(|| {
                    black_box(&distances)
                        .iter()
                        .map(|&scaled| lut.at(scaled))
                        .sum::<f32>()
                });
            },
        );
    }
    group.finish();
}
