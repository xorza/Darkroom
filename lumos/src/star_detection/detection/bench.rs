//! Benchmarks for star candidate detection.

use super::{LabelMap, extract_candidates};
use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::config::DeblendConfig;
use crate::testing::synthetic::stamps::benchmark_star_field;
use ::bench::quick_bench;
use std::hint::black_box;

/// Create a threshold mask from a star field image.
fn create_test_mask(pixels: &Buffer2<f32>, threshold: f32) -> BitBuffer2 {
    let width = pixels.width();
    let height = pixels.height();
    let mut mask = BitBuffer2::new_filled(width, height, false);

    for (idx, &value) in pixels.iter().enumerate() {
        if value > threshold {
            mask.set(idx, true);
        }
    }

    mask
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_extract_candidates_1k_sparse(b: ::bench::Bencher) {
    // 1K image with ~100 stars (sparse field)
    let pixels = benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 42);
    let mask = create_test_mask(&pixels, 0.05);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            black_box(500),
        ))
    });
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_extract_candidates_1k_dense(b: ::bench::Bencher) {
    // 1K image with ~500 stars (dense field)
    let pixels = benchmark_star_field(1024, 1024, 500, 0.1, 0.01, 42);
    let mask = create_test_mask(&pixels, 0.05);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            black_box(500),
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_sparse(b: ::bench::Bencher) {
    // 4K image with ~500 stars
    let pixels = benchmark_star_field(4096, 4096, 500, 0.1, 0.01, 42);
    let mask = create_test_mask(&pixels, 0.05);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            black_box(500),
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_dense(b: ::bench::Bencher) {
    // 4K image with ~2000 stars (crowded field)
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_test_mask(&pixels, 0.05);
    let label_map = LabelMap::from_mask(&mask);
    let config = DeblendConfig::default();

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            black_box(500),
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_dense_multithreshold(b: ::bench::Bencher) {
    // 4K image with ~2000 stars (crowded field)
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_test_mask(&pixels, 0.05);
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
            black_box(500),
        ))
    });
}

// Benchmark LabelMap::from_mask separately to understand the breakdown
#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_label_map_from_mask_1k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(1024, 1024, 500, 0.1, 0.01, 42);
    let mask = create_test_mask(&pixels, 0.05);

    b.bench(|| black_box(LabelMap::from_mask(black_box(&mask))));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_label_map_from_mask_4k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_test_mask(&pixels, 0.05);

    b.bench(|| black_box(LabelMap::from_mask(black_box(&mask))));
}
