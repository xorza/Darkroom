//! Benchmark module for mean stacking.
//! Run with: cargo bench --package lumos --features bench stack_mean

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

/// Register mean stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {
    benchmark_accumulate(c);
}

/// Benchmark the accumulate operation.
fn benchmark_accumulate(c: &mut Criterion) {
    let mut group = c.benchmark_group("mean_accumulate");

    for size in [1024, 4096, 16384] {
        let src: Vec<f32> = (0..size).map(|x| x as f32 * 0.001).collect();

        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(BenchmarkId::new("accumulate", size), |b| {
            let mut dst: Vec<f32> = vec![0.0; size];
            b.iter(|| {
                for (d, &s) in dst.iter_mut().zip(src.iter()) {
                    *d += s;
                }
                black_box(&dst);
            })
        });
    }

    group.finish();
}
