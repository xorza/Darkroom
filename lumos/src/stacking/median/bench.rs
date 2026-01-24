//! Benchmark module for median stacking.
//! Run with: cargo bench --package lumos --features bench stack_median

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

/// Register median stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {
    benchmark_median_calculation(c);
}

/// Benchmark the core median calculation.
fn benchmark_median_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("median_calculation");

    // Realistic astrophotography stack sizes (20-100+ frames typical)
    for size in [20, 50, 100] {
        // Create test data - reversed sequence is worst case for sorting
        let values: Vec<f32> = (1..=size).map(|x| x as f32).rev().collect();

        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(BenchmarkId::new("quickselect", size), |b| {
            b.iter(|| {
                let result = crate::math::median_f32(black_box(&values));
                black_box(result)
            })
        });
    }

    group.finish();
}
