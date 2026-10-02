//! Benchmarks for row conversion operations (SIMD vs Scalar), over every ordered pair of
//! distinct formats.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::common::color_format::ALL_FORMATS;
use crate::common::internals::create_test_image;
use crate::image::conversion::{scalar, simd};

const WIDTH_4K: usize = 4096;

pub fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("conversion/row");
    group.sample_size(50);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));

    for from in ALL_FORMATS {
        let source = create_test_image(from, WIDTH_4K, 1, 0);
        for to in ALL_FORMATS {
            if from == to {
                continue;
            }
            let src = source.bytes();
            let mut dst = vec![0u8; WIDTH_4K * to.byte_count()];
            // Criterion turns ids into report paths, so keep them space-free.
            let label = format!("{from}_to_{to}").replace(' ', "_");
            group.throughput(Throughput::Bytes(src.len() as u64));

            if let Some(kernel) = simd::row_converter(from, to) {
                group.bench_function(BenchmarkId::new("simd", &label), |b| {
                    // SAFETY: `row_converter` verified this CPU has the kernel's feature.
                    b.iter(|| unsafe {
                        kernel(black_box(src), black_box(&mut dst), black_box(WIDTH_4K));
                    });
                });
            }

            let convert_row = scalar::row_converter(from, to);
            group.bench_function(BenchmarkId::new("scalar", &label), |b| {
                b.iter(|| convert_row(black_box(src), black_box(&mut dst), black_box(WIDTH_4K)));
            });
        }
    }

    group.finish();
}
