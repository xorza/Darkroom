//! Benchmark module for median stacking.
//! Run with: cargo bench --package lumos --features bench median

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::stacking::FrameType;
use crate::stacking::median::{MedianStackConfig, stack_median_from_paths};

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
                let result = super::cpu::median_f32(black_box(&values));
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark full median stacking pipeline with real images.
#[allow(dead_code)]
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

    let mut group = c.benchmark_group("median_stacking");
    group.sample_size(10);

    let config = MedianStackConfig {
        chunk_rows: 128,
        cache_dir: std::env::temp_dir().join("lumos_bench_cache"),
        keep_cache: false,
    };

    group.bench_function(
        BenchmarkId::new("stack_median", format!("{}frames", frame_count)),
        |b| b.iter(|| black_box(stack_median_from_paths(&paths, FrameType::Dark, &config))),
    );

    group.finish();
}
