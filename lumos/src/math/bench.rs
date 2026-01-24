//! Benchmark module for math operations.
//! Run with: cargo bench --package lumos --features bench stack_math

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

/// Register math benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {
    benchmark_sum_f32(c);
    benchmark_sum_squared_diff(c);
}

/// Benchmark sum_f32 comparing scalar vs SIMD.
fn benchmark_sum_f32(c: &mut Criterion) {
    let mut group = c.benchmark_group("sum_f32");

    for size in [64, 256, 1024, 4096] {
        let values: Vec<f32> = (0..size).map(|x| x as f32 * 0.001).collect();

        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(BenchmarkId::new("scalar", size), |b| {
            b.iter(|| {
                let result = super::scalar::sum_f32(black_box(&values));
                black_box(result)
            })
        });

        #[cfg(target_arch = "aarch64")]
        group.bench_function(BenchmarkId::new("neon", size), |b| {
            b.iter(|| {
                let result = unsafe { super::neon::sum_f32(black_box(&values)) };
                black_box(result)
            })
        });

        #[cfg(target_arch = "x86_64")]
        if crate::common::cpu_features::has_sse4_1() {
            group.bench_function(BenchmarkId::new("sse", size), |b| {
                b.iter(|| {
                    let result = unsafe { super::sse::sum_f32(black_box(&values)) };
                    black_box(result)
                })
            });
        }
    }

    group.finish();
}

/// Benchmark sum_squared_diff comparing scalar vs SIMD.
fn benchmark_sum_squared_diff(c: &mut Criterion) {
    let mut group = c.benchmark_group("sum_squared_diff");

    for size in [64, 256, 1024, 4096] {
        let values: Vec<f32> = (0..size).map(|x| x as f32 * 0.001).collect();
        let mean = values.iter().sum::<f32>() / values.len() as f32;

        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(BenchmarkId::new("scalar", size), |b| {
            b.iter(|| {
                let result = super::scalar::sum_squared_diff(black_box(&values), black_box(mean));
                black_box(result)
            })
        });

        #[cfg(target_arch = "aarch64")]
        group.bench_function(BenchmarkId::new("neon", size), |b| {
            b.iter(|| {
                let result =
                    unsafe { super::neon::sum_squared_diff(black_box(&values), black_box(mean)) };
                black_box(result)
            })
        });

        #[cfg(target_arch = "x86_64")]
        if crate::common::cpu_features::has_sse4_1() {
            group.bench_function(BenchmarkId::new("sse", size), |b| {
                b.iter(|| {
                    let result = unsafe {
                        super::sse::sum_squared_diff(black_box(&values), black_box(mean))
                    };
                    black_box(result)
                })
            });
        }
    }

    group.finish();
}
