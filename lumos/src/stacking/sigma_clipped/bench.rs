//! Benchmark module for sigma-clipped mean stacking.
//! Run with: cargo bench --package lumos --features bench sigma_clipped

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion};

use crate::stacking::sigma_clipped::{SigmaClippedConfig, stack_sigma_clipped_from_paths};
use crate::stacking::{FrameType, SigmaClipConfig};

/// Register sigma-clipped mean stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, calibration_dir: &Path) {
    let darks_dir = calibration_dir.join("Darks");
    if !darks_dir.exists() {
        eprintln!("Darks directory not found, skipping sigma-clipped stacking benchmarks");
        return;
    }

    let paths = common::file_utils::astro_image_files(&darks_dir);
    if paths.is_empty() {
        eprintln!("No dark files found, skipping sigma-clipped stacking benchmarks");
        return;
    }

    // Limit to first 10 frames for reasonable benchmark time
    let paths: Vec<_> = paths.into_iter().take(10).collect();
    let frame_count = paths.len();

    let mut group = c.benchmark_group("sigma_clipped_stacking");
    group.sample_size(10);

    // Benchmark with different sigma values
    for sigma in [2.0, 2.5, 3.0] {
        let config = SigmaClippedConfig {
            clip: SigmaClipConfig::new(sigma, 3),
            chunk_rows: 128,
            cache_dir: std::env::temp_dir().join("lumos_bench_cache"),
            keep_cache: false,
        };

        group.bench_function(
            BenchmarkId::new(
                "stack_sigma_clipped",
                format!("{}frames_sigma{}", frame_count, sigma),
            ),
            |b| {
                b.iter(|| {
                    black_box(stack_sigma_clipped_from_paths(
                        &paths,
                        FrameType::Dark,
                        &config,
                    ))
                })
            },
        );
    }

    group.finish();
}
