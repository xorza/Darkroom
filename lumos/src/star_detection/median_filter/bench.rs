//! Benchmarks for 3x3 median filter.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group};

use super::median_filter_3x3;

pub fn bench_median_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("median_filter_3x3");

    for (width, height) in [(512, 512), (1024, 1024), (4096, 4096)] {
        let pixels: Vec<f32> = (0..width * height)
            .map(|i| (i % 256) as f32 / 255.0)
            .collect();

        group.throughput(Throughput::Elements((width * height) as u64));
        group.bench_with_input(
            BenchmarkId::new("size", format!("{}x{}", width, height)),
            &pixels,
            |b, pixels| {
                b.iter(|| {
                    black_box(median_filter_3x3(black_box(pixels), width, height));
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_median_filter);
