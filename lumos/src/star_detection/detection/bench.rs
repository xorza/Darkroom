//! Benchmark module for star detection algorithms.
//! Run with: cargo bench --package lumos --features bench --bench star_detection_detection

use super::{
    connected_components, create_threshold_mask, detect_stars, detect_stars_filtered,
    extract_candidates,
};
use crate::common::Buffer2;
use crate::star_detection::StarDetectionConfig;
use crate::star_detection::constants::dilate_mask;
use crate::star_detection::deblend::DeblendConfig;
use crate::testing::synthetic::{background_map, stamps};
use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// Register detection benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    const WIDTH: usize = 6144;
    const HEIGHT: usize = 6144;
    const NUM_STARS: usize = 3000;

    let pixels = stamps::benchmark_star_field(WIDTH, HEIGHT, NUM_STARS, 0.1, 0.01, 42);
    let background = background_map::uniform(WIDTH, HEIGHT, 0.1, 0.01);
    let size_name = format!("{}x{}_{}stars", WIDTH, HEIGHT, NUM_STARS);

    // Threshold mask creation benchmark
    let mut mask_group = c.benchmark_group("threshold_mask");
    mask_group.sample_size(10);
    mask_group.throughput(Throughput::Elements((WIDTH * HEIGHT) as u64));

    let mut mask = Buffer2::new_filled(WIDTH, HEIGHT, false);
    mask_group.bench_function(BenchmarkId::new("create_threshold_mask", &size_name), |b| {
        b.iter(|| {
            create_threshold_mask(
                black_box(&pixels),
                black_box(&background),
                black_box(3.0),
                black_box(&mut mask),
            );
        })
    });

    mask_group.finish();

    // Dilation benchmark
    let mut dilate_group = c.benchmark_group("dilate_mask");
    dilate_group.sample_size(10);
    dilate_group.throughput(Throughput::Elements((WIDTH * HEIGHT) as u64));

    let mut mask = Buffer2::new_filled(WIDTH, HEIGHT, false);
    create_threshold_mask(&pixels, &background, 3.0, &mut mask);
    let mut output = Buffer2::new_filled(WIDTH, HEIGHT, false);
    dilate_group.bench_function(BenchmarkId::new(&size_name, "radius_1"), |b| {
        b.iter(|| dilate_mask(black_box(&mask), black_box(1), black_box(&mut output)))
    });

    dilate_group.finish();

    // Connected components benchmark
    let mut cc_group = c.benchmark_group("connected_components");
    cc_group.sample_size(10);
    cc_group.throughput(Throughput::Elements((WIDTH * HEIGHT) as u64));

    let mut dilated_mask = Buffer2::new_filled(WIDTH, HEIGHT, false);
    create_threshold_mask(&pixels, &background, 3.0, &mut dilated_mask);
    let mut dilated = Buffer2::new_filled(WIDTH, HEIGHT, false);
    dilate_mask(&dilated_mask, 1, &mut dilated);
    std::mem::swap(&mut dilated_mask, &mut dilated);

    cc_group.bench_function(BenchmarkId::new("connected_components", &size_name), |b| {
        b.iter(|| black_box(connected_components(black_box(&dilated_mask))))
    });

    cc_group.finish();

    // Extract candidates benchmark
    let mut extract_group = c.benchmark_group("extract_candidates");
    extract_group.sample_size(10);

    let (labels, num_labels) = connected_components(&dilated_mask);

    let deblend_config = DeblendConfig {
        min_separation: 3,
        min_prominence: 0.1,
        multi_threshold: false,
        n_thresholds: 32,
        min_contrast: 0.005,
    };

    extract_group.bench_function(
        BenchmarkId::new("extract_candidates_simple", &size_name),
        |b| {
            b.iter(|| {
                black_box(extract_candidates(
                    black_box(&pixels),
                    black_box(&labels),
                    black_box(num_labels),
                    black_box(&deblend_config),
                    black_box(500),
                ))
            })
        },
    );

    let mt_deblend_config = DeblendConfig {
        min_separation: 3,
        min_prominence: 0.1,
        multi_threshold: true,
        n_thresholds: 32,
        min_contrast: 0.005,
    };

    extract_group.bench_function(
        BenchmarkId::new("extract_candidates_multi_threshold", &size_name),
        |b| {
            b.iter(|| {
                black_box(extract_candidates(
                    black_box(&pixels),
                    black_box(&labels),
                    black_box(num_labels),
                    black_box(&mt_deblend_config),
                    black_box(500),
                ))
            })
        },
    );

    extract_group.finish();

    // Full detect_stars benchmark
    let mut detect_group = c.benchmark_group("detect_stars");
    detect_group.sample_size(10);
    detect_group.throughput(Throughput::Elements((WIDTH * HEIGHT) as u64));

    let config = StarDetectionConfig::default();

    detect_group.bench_function(BenchmarkId::new("detect_stars", &size_name), |b| {
        b.iter(|| {
            black_box(detect_stars(
                black_box(&pixels),
                black_box(&background),
                black_box(&config),
            ))
        })
    });

    detect_group.finish();

    // detect_stars_filtered benchmark
    let mut filtered_group = c.benchmark_group("detect_stars_filtered");
    filtered_group.sample_size(10);
    filtered_group.throughput(Throughput::Elements((WIDTH * HEIGHT) as u64));

    let filtered = stamps::benchmark_star_field(WIDTH, HEIGHT, NUM_STARS, 0.1, 0.01, 43);

    filtered_group.bench_function(BenchmarkId::new("detect_stars_filtered", &size_name), |b| {
        b.iter(|| {
            black_box(detect_stars_filtered(
                black_box(&pixels),
                black_box(&filtered),
                black_box(&background),
                black_box(&config),
            ))
        })
    });

    filtered_group.finish();
}
