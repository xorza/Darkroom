//! Benchmark module for sigma-clipped mean stacking.
//! Run with: cargo bench -p lumos --features bench --bench stack_sigma_clipped

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::stacking::SigmaClipConfig;

/// Register sigma-clipped stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    benchmark_sigma_clipped_mean(c);
}

/// Benchmark the sigma-clipped mean calculation.
fn benchmark_sigma_clipped_mean(c: &mut Criterion) {
    let mut group = c.benchmark_group("sigma_clipped_mean");
    let config = SigmaClipConfig::new(2.5, 3);

    // Test various stack sizes typical for astrophotography
    for size in [20, 50, 100] {
        // Create test data with one outlier
        let original: Vec<f32> = {
            let mut v: Vec<f32> = (1..=size).map(|x| 10.0 + (x as f32 * 0.01)).collect();
            v[0] = 1000.0; // Add outlier
            v
        };

        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(BenchmarkId::new("with_outlier", size), |b| {
            let mut values = original.clone();
            b.iter(|| {
                values.copy_from_slice(&original);
                let result = super::sigma_clipped_mean(black_box(&mut values), black_box(&config));
                black_box(result)
            })
        });
    }

    group.finish();
}
