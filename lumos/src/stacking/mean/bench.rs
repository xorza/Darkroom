//! Benchmark module for mean stacking.
//! Run with: cargo bench --package lumos --features bench stack_mean

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

/// Register mean stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {
    benchmark_accumulate(c);
    benchmark_divide(c);
}

/// Benchmark the accumulate operation.
fn benchmark_accumulate(c: &mut Criterion) {
    let mut group = c.benchmark_group("mean_accumulate");

    for size in [1024, 4096, 16384] {
        let src: Vec<f32> = (0..size).map(|x| x as f32 * 0.001).collect();

        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(BenchmarkId::new("scalar", size), |b| {
            let mut dst: Vec<f32> = vec![0.0; size];
            b.iter(|| {
                super::scalar::accumulate_chunk(black_box(&mut dst), black_box(&src));
            })
        });
    }

    group.finish();
}

/// Benchmark the divide operation.
fn benchmark_divide(c: &mut Criterion) {
    let mut group = c.benchmark_group("mean_divide");

    for size in [1024, 4096, 16384] {
        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(BenchmarkId::new("scalar", size), |b| {
            let mut data: Vec<f32> = (0..size).map(|x| x as f32).collect();
            b.iter(|| {
                super::scalar::divide_chunk(black_box(&mut data), black_box(0.1));
            })
        });
    }

    group.finish();
}
