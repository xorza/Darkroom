//! Benchmark module for mean stacking.
//! Run with: cargo bench --package lumos --features bench stack_mean

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::stacking::FrameType;
use crate::stacking::mean::stack_mean_from_paths;

/// Register mean stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, calibration_dir: &Path) {
    // Benchmark the core accumulate/divide operations (scalar vs SIMD)
    benchmark_accumulate(c);
    benchmark_divide(c);

    // Benchmark full stacking pipeline
    benchmark_full_stacking(c, calibration_dir);
}

/// Benchmark the accumulate operation comparing scalar vs SIMD.
fn benchmark_accumulate(c: &mut Criterion) {
    let mut group = c.benchmark_group("mean_accumulate");

    // Test various chunk sizes
    for size in [64, 256, 1024, 4096, 16384] {
        let src: Vec<f32> = (0..size).map(|x| x as f32 * 0.001).collect();

        group.throughput(Throughput::Elements(size as u64));

        // Benchmark scalar implementation
        group.bench_function(BenchmarkId::new("scalar", size), |b| {
            let mut dst: Vec<f32> = vec![0.0; size];
            b.iter(|| {
                super::scalar::accumulate_chunk(black_box(&mut dst), black_box(&src));
            })
        });

        // Benchmark SIMD implementation
        #[cfg(target_arch = "aarch64")]
        group.bench_function(BenchmarkId::new("neon", size), |b| {
            let mut dst: Vec<f32> = vec![0.0; size];
            b.iter(|| unsafe {
                super::neon::accumulate_chunk(black_box(&mut dst), black_box(&src));
            })
        });

        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("sse2") {
                group.bench_function(BenchmarkId::new("sse", size), |b| {
                    let mut dst: Vec<f32> = vec![0.0; size];
                    b.iter(|| unsafe {
                        super::sse::accumulate_chunk(black_box(&mut dst), black_box(&src));
                    })
                });
            }
        }
    }

    group.finish();
}

/// Benchmark the divide operation comparing scalar vs SIMD.
fn benchmark_divide(c: &mut Criterion) {
    let mut group = c.benchmark_group("mean_divide");

    // Test various chunk sizes
    for size in [64, 256, 1024, 4096, 16384] {
        group.throughput(Throughput::Elements(size as u64));

        // Benchmark scalar implementation
        group.bench_function(BenchmarkId::new("scalar", size), |b| {
            let mut data: Vec<f32> = (0..size).map(|x| x as f32).collect();
            b.iter(|| {
                super::scalar::divide_chunk(black_box(&mut data), black_box(0.1));
            })
        });

        // Benchmark SIMD implementation
        #[cfg(target_arch = "aarch64")]
        group.bench_function(BenchmarkId::new("neon", size), |b| {
            let mut data: Vec<f32> = (0..size).map(|x| x as f32).collect();
            b.iter(|| unsafe {
                super::neon::divide_chunk(black_box(&mut data), black_box(0.1));
            })
        });

        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("sse2") {
                group.bench_function(BenchmarkId::new("sse", size), |b| {
                    let mut data: Vec<f32> = (0..size).map(|x| x as f32).collect();
                    b.iter(|| unsafe {
                        super::sse::divide_chunk(black_box(&mut data), black_box(0.1));
                    })
                });
            }
        }
    }

    group.finish();
}

/// Benchmark full mean stacking pipeline with real images.
fn benchmark_full_stacking(c: &mut Criterion, calibration_dir: &Path) {
    let darks_dir = calibration_dir.join("Darks");
    if !darks_dir.exists() {
        eprintln!("Darks directory not found, skipping full stacking benchmarks");
        return;
    }

    let paths = common::file_utils::astro_image_files(&darks_dir);
    if paths.is_empty() {
        eprintln!("No dark files found, skipping full stacking benchmarks");
        return;
    }

    // Limit to first 10 frames for reasonable benchmark time
    let paths: Vec<_> = paths.into_iter().take(10).collect();
    let frame_count = paths.len();

    let mut group = c.benchmark_group("mean_stacking");
    group.sample_size(10);

    group.bench_function(
        BenchmarkId::new("stack_mean", format!("{}frames", frame_count)),
        |b| b.iter(|| black_box(stack_mean_from_paths(&paths, FrameType::Dark))),
    );

    group.finish();
}
