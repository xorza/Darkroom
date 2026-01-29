//! Benchmark module for star detection algorithms.
//! Run with: cargo bench --package lumos --features bench --bench star_detection_detection

use super::{
    connected_components, create_threshold_mask, detect_stars, detect_stars_filtered,
    extract_candidates,
};
use crate::common::Buffer2;
use crate::star_detection::StarDetectionConfig;
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::constants::dilate_mask;
use crate::star_detection::deblend::DeblendConfig;
use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// Generate a synthetic star field image for benchmarking.
fn generate_test_image(width: usize, height: usize, num_stars: usize) -> Buffer2<f32> {
    let background = 0.1f32;
    let mut pixels = vec![background; width * height];

    // Add deterministic noise
    for (i, p) in pixels.iter_mut().enumerate() {
        let hash = ((i as u32).wrapping_mul(2654435761)) as f32 / u32::MAX as f32;
        *p += (hash - 0.5) * 0.02;
    }

    // Add synthetic stars at deterministic positions
    for star_idx in 0..num_stars {
        let hash1 = ((star_idx as u32).wrapping_mul(2654435761)) as usize;
        let hash2 = ((star_idx as u32).wrapping_mul(1597334677)) as usize;
        let hash3 = ((star_idx as u32).wrapping_mul(805306457)) as usize;
        let hash4 = ((star_idx as u32).wrapping_mul(402653189)) as usize;

        let cx = 15 + (hash1 % (width - 30));
        let cy = 15 + (hash2 % (height - 30));
        let brightness = 0.5 + (hash3 % 500) as f32 / 1000.0;
        let sigma = 1.5 + (hash4 % 100) as f32 / 100.0;

        for dy in -8i32..=8 {
            for dx in -8i32..=8 {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                if x < width && y < height {
                    let r2 = (dx * dx + dy * dy) as f32;
                    let value = brightness * (-r2 / (2.0 * sigma * sigma)).exp();
                    pixels[y * width + x] += value;
                }
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

/// Create a background map for benchmarking.
fn create_background_map(width: usize, height: usize) -> BackgroundMap {
    BackgroundMap {
        background: Buffer2::new_filled(width, height, 0.1f32),
        noise: Buffer2::new_filled(width, height, 0.01f32),
    }
}

/// Register detection benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    const WIDTH: usize = 6144;
    const HEIGHT: usize = 6144;
    const NUM_STARS: usize = 3000;

    let pixels = generate_test_image(WIDTH, HEIGHT, NUM_STARS);
    let background = create_background_map(WIDTH, HEIGHT);
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

    let filtered = generate_test_image(WIDTH, HEIGHT, NUM_STARS);

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
