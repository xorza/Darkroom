//! Benchmark module for median stacking.
//! Run with: cargo bench --package lumos --features bench median

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion};

use crate::stacking::FrameType;
use crate::stacking::median::{MedianStackConfig, stack_median_from_paths};

/// Register median stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, calibration_dir: &Path) {
    let darks_dir = calibration_dir.join("Darks");
    if !darks_dir.exists() {
        eprintln!("Darks directory not found, skipping median stacking benchmarks");
        return;
    }

    let paths = common::file_utils::astro_image_files(&darks_dir);
    if paths.is_empty() {
        eprintln!("No dark files found, skipping median stacking benchmarks");
        return;
    }

    // Limit to first 10 frames for reasonable benchmark time
    let paths: Vec<_> = paths.into_iter().take(10).collect();
    let frame_count = paths.len();

    let mut group = c.benchmark_group("median_stacking");
    group.sample_size(10);

    // Benchmark with different chunk sizes
    for chunk_rows in [32, 64, 128] {
        let config = MedianStackConfig {
            chunk_rows,
            cache_dir: std::env::temp_dir().join("lumos_bench_cache"),
            keep_cache: false,
        };

        group.bench_function(
            BenchmarkId::new(
                "stack_median",
                format!("{}frames_{}rows", frame_count, chunk_rows),
            ),
            |b| b.iter(|| black_box(stack_median_from_paths(&paths, FrameType::Dark, &config))),
        );
    }

    group.finish();
}
