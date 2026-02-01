//! Benchmarks for star candidate detection.

use super::{LabelMap, extract_candidates};
use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::star_detection::common::{dilate_mask, threshold_mask::create_threshold_mask};
use crate::star_detection::config::DeblendConfig;
use crate::testing::init_tracing;
use crate::testing::synthetic::stamps::benchmark_star_field;
use ::bench::quick_bench;
use std::hint::black_box;

/// Create a threshold mask using the real detection pipeline.
/// Uses background estimation, sigma thresholding, and dilation.
fn create_detection_mask(pixels: &Buffer2<f32>, sigma_threshold: f32) -> BitBuffer2 {
    let width = pixels.width();
    let height = pixels.height();

    // Create background map (same as real pipeline)
    let background = BackgroundMap::new(pixels, &BackgroundConfig::default());

    // Create threshold mask
    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask(
        pixels.pixels(),
        background.background.pixels(),
        background.noise.pixels(),
        sigma_threshold,
        &mut mask,
    );

    // Dilate mask (same as real pipeline - radius 1)
    let mut dilated = BitBuffer2::new_filled(width, height, false);
    dilate_mask(&mask, 1, &mut dilated);

    dilated
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_extract_candidates_1k_sparse(b: ::bench::Bencher) {
    // 1K image with ~100 stars (sparse field)
    let pixels = benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_extract_candidates_1k_dense(b: ::bench::Bencher) {
    // 1K image with ~500 stars (dense field)
    let pixels = benchmark_star_field(1024, 1024, 500, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_sparse(b: ::bench::Bencher) {
    // 4K image with ~500 stars
    let pixels = benchmark_star_field(4096, 4096, 500, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_dense(b: ::bench::Bencher) {
    // 4K image with ~2000 stars (crowded field)
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_dense_multithreshold(b: ::bench::Bencher) {
    // 4K image with ~2000 stars (crowded field)
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig {
        n_thresholds: 32,
        ..Default::default()
    };

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 3, iters = 430)]
fn bench_extract_candidates_6k_dense(b: ::bench::Bencher) {
    init_tracing();

    // 6K image with ~10000 stars (dense field)
    let pixels = benchmark_star_field(6144, 6144, 50000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 10)]
fn bench_extract_candidates_6k_globular_cluster_multithreshold(b: ::bench::Bencher) {
    init_tracing();

    use crate::testing::synthetic::generate_globular_cluster;

    // 6K globular cluster with 50000 stars - extreme crowding
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig {
        n_thresholds: 32,
        max_area: 5000,
        ..Default::default()
    };

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
        ))
    });
}

// Benchmark LabelMap::from_mask separately to understand the breakdown
#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_label_map_from_mask_1k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(1024, 1024, 500, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);

    b.bench(|| black_box(LabelMap::from_mask(black_box(&mask))));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_label_map_from_mask_4k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);

    b.bench(|| black_box(LabelMap::from_mask(black_box(&mask))));
}

#[quick_bench(warmup_iters = 1, iters = 10)]
fn bench_label_map_from_mask_6k_globular(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(4096, 4096, 50000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);

    b.bench(|| black_box(LabelMap::from_mask(black_box(&mask))));
}
