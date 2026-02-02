//! Benchmarks for star candidate detection.

use super::{detect_stars, extract_candidates, label_map_from_mask_with_connectivity};
use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::star_detection::buffer_pool::BufferPool;
use crate::star_detection::config::{DeblendConfig, FilteringConfig, StarDetectionConfig};
use crate::star_detection::mask_dilation::dilate_mask;
use crate::star_detection::threshold_mask::create_threshold_mask;
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
    let background = crate::testing::estimate_background(pixels, BackgroundConfig::default());

    // Create threshold mask
    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask(
        pixels,
        &background.background,
        &background.noise,
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
    let label_map = label_map_from_mask_with_connectivity(
        &mask,
        crate::star_detection::config::Connectivity::Four,
    );
    let config = DeblendConfig::default();
    let max_area = FilteringConfig::default().max_area;

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            max_area,
        ))
    });
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_extract_candidates_1k_dense(b: ::bench::Bencher) {
    // 1K image with ~500 stars (dense field)
    let pixels = benchmark_star_field(1024, 1024, 500, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = label_map_from_mask_with_connectivity(
        &mask,
        crate::star_detection::config::Connectivity::Four,
    );
    let config = DeblendConfig::default();
    let max_area = FilteringConfig::default().max_area;

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            max_area,
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_sparse(b: ::bench::Bencher) {
    // 4K image with ~500 stars
    let pixels = benchmark_star_field(4096, 4096, 500, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = label_map_from_mask_with_connectivity(
        &mask,
        crate::star_detection::config::Connectivity::Four,
    );
    let config = DeblendConfig::default();
    let max_area = FilteringConfig::default().max_area;

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            max_area,
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_dense(b: ::bench::Bencher) {
    // 4K image with ~2000 stars (crowded field)
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = label_map_from_mask_with_connectivity(
        &mask,
        crate::star_detection::config::Connectivity::Four,
    );
    let config = DeblendConfig::default();
    let max_area = FilteringConfig::default().max_area;

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            max_area,
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_extract_candidates_4k_dense_multithreshold(b: ::bench::Bencher) {
    // 4K image with ~2000 stars (crowded field)
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = label_map_from_mask_with_connectivity(
        &mask,
        crate::star_detection::config::Connectivity::Four,
    );
    let config = DeblendConfig {
        n_thresholds: 32,
        ..Default::default()
    };
    let max_area = FilteringConfig::default().max_area;

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            max_area,
        ))
    });
}

#[quick_bench(warmup_iters = 3, iters = 50)]
fn bench_extract_candidates_6k_dense(b: ::bench::Bencher) {
    init_tracing();

    // 6K image with ~10000 stars (dense field)
    let pixels = benchmark_star_field(6144, 6144, 50000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let label_map = label_map_from_mask_with_connectivity(
        &mask,
        crate::star_detection::config::Connectivity::Four,
    );

    let crowded_config = StarDetectionConfig::for_crowded_field();
    let config = &crowded_config.deblend;
    let max_area = crowded_config.filtering.max_area;

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(config),
            max_area,
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
    let label_map = label_map_from_mask_with_connectivity(
        &mask,
        crate::star_detection::config::Connectivity::Four,
    );
    let config = DeblendConfig {
        n_thresholds: 32,
        ..Default::default()
    };
    let max_area = 5000;

    b.bench(|| {
        black_box(extract_candidates(
            black_box(&pixels),
            black_box(&label_map),
            black_box(&config),
            max_area,
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 40)]
fn bench_detect_stars_6k_50000(b: ::bench::Bencher) {
    init_tracing();

    // 6K image with 50000 stars - full detect_stars pipeline
    let pixels = benchmark_star_field(6144, 6144, 50000, 0.1, 0.01, 42);
    let background = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let config = StarDetectionConfig::for_crowded_field();
    let mut pool = BufferPool::new(pixels.width(), pixels.height());

    b.bench(|| {
        black_box(detect_stars(
            black_box(&pixels),
            black_box(None),
            black_box(&background),
            black_box(&config),
            &mut pool,
        ))
    });
}
