//! Benchmark module for star detection algorithms.
//! Run with: cargo bench --package lumos --features bench detection

use super::simd::create_threshold_mask_simd;
use super::{
    connected_components, create_threshold_mask, detect_stars, detect_stars_filtered, dilate_mask,
    extract_candidates,
};
use crate::star_detection::StarDetectionConfig;
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::deblend::DeblendConfig;
use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// Generate a synthetic star field image for benchmarking.
fn generate_test_image(width: usize, height: usize, num_stars: usize) -> Vec<f32> {
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

    pixels
}

/// Create a background map for benchmarking.
fn create_background_map(width: usize, height: usize) -> BackgroundMap {
    let size = width * height;
    BackgroundMap {
        background: vec![0.1f32; size],
        noise: vec![0.01f32; size],
        width,
        height,
    }
}

/// Register detection benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    // Threshold mask creation benchmarks
    let mut mask_group = c.benchmark_group("threshold_mask");
    mask_group.sample_size(30);

    for &(width, height) in &[(512, 512), (1024, 1024), (2048, 2048)] {
        let pixels = generate_test_image(width, height, 100);
        let background = create_background_map(width, height);
        let size_name = format!("{}x{}", width, height);

        mask_group.throughput(Throughput::Elements((width * height) as u64));

        mask_group.bench_function(BenchmarkId::new("create_threshold_mask", &size_name), |b| {
            b.iter(|| {
                black_box(create_threshold_mask(
                    black_box(&pixels),
                    black_box(&background),
                    black_box(3.0),
                ))
            })
        });
    }

    mask_group.finish();

    // SIMD vs Scalar comparison for threshold mask
    let mut simd_group = c.benchmark_group("threshold_mask_simd_vs_scalar");
    simd_group.sample_size(30);

    for &(width, height) in &[(512, 512), (1024, 1024), (2048, 2048)] {
        let pixels = generate_test_image(width, height, 100);
        let background = create_background_map(width, height);
        let size_name = format!("{}x{}", width, height);

        simd_group.throughput(Throughput::Elements((width * height) as u64));

        // Scalar implementation
        simd_group.bench_function(BenchmarkId::new("scalar", &size_name), |b| {
            b.iter(|| {
                black_box(create_threshold_mask(
                    black_box(&pixels),
                    black_box(&background),
                    black_box(3.0),
                ))
            })
        });

        // SIMD implementation
        simd_group.bench_function(BenchmarkId::new("simd", &size_name), |b| {
            b.iter(|| {
                black_box(create_threshold_mask_simd(
                    black_box(&pixels),
                    black_box(&background),
                    black_box(3.0),
                ))
            })
        });
    }

    simd_group.finish();

    // Dilation benchmarks
    let mut dilate_group = c.benchmark_group("dilate_mask");
    dilate_group.sample_size(30);

    for &(width, height) in &[(512, 512), (1024, 1024), (2048, 2048)] {
        let pixels = generate_test_image(width, height, 100);
        let background = create_background_map(width, height);
        let mask = create_threshold_mask(&pixels, &background, 3.0);
        let size_name = format!("{}x{}", width, height);

        dilate_group.throughput(Throughput::Elements((width * height) as u64));

        for radius in [1, 2, 3] {
            dilate_group.bench_function(
                BenchmarkId::new(&size_name, format!("radius_{}", radius)),
                |b| {
                    b.iter(|| {
                        black_box(dilate_mask(
                            black_box(&mask),
                            black_box(width),
                            black_box(height),
                            black_box(radius),
                        ))
                    })
                },
            );
        }
    }

    dilate_group.finish();

    // Connected components benchmarks
    let mut cc_group = c.benchmark_group("connected_components");
    cc_group.sample_size(30);

    for &(width, height, num_stars) in &[
        (512, 512, 50),
        (1024, 1024, 200),
        (2048, 2048, 800),
        (4096, 4096, 3200),
    ] {
        let pixels = generate_test_image(width, height, num_stars);
        let background = create_background_map(width, height);
        let mask = create_threshold_mask(&pixels, &background, 3.0);
        let mask = dilate_mask(&mask, width, height, 1);
        let size_name = format!("{}x{}_{}stars", width, height, num_stars);

        cc_group.throughput(Throughput::Elements((width * height) as u64));

        cc_group.bench_function(BenchmarkId::new("connected_components", &size_name), |b| {
            b.iter(|| {
                black_box(connected_components(
                    black_box(&mask),
                    black_box(width),
                    black_box(height),
                ))
            })
        });
    }

    cc_group.finish();

    // Extract candidates benchmarks
    let mut extract_group = c.benchmark_group("extract_candidates");
    extract_group.sample_size(30);

    for &(width, height, num_stars) in &[(512, 512, 50), (1024, 1024, 200)] {
        let pixels = generate_test_image(width, height, num_stars);
        let background = create_background_map(width, height);
        let mask = create_threshold_mask(&pixels, &background, 3.0);
        let mask = dilate_mask(&mask, width, height, 1);
        let (labels, num_labels) = connected_components(&mask, width, height);
        let size_name = format!("{}x{}_{}stars", width, height, num_stars);

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
                        black_box(width),
                        black_box(height),
                        black_box(&deblend_config),
                    ))
                })
            },
        );

        // Multi-threshold deblending
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
                        black_box(width),
                        black_box(height),
                        black_box(&mt_deblend_config),
                    ))
                })
            },
        );
    }

    extract_group.finish();

    // Full detect_stars benchmarks
    let mut detect_group = c.benchmark_group("detect_stars");
    detect_group.sample_size(20);

    for &(width, height, num_stars) in &[(512, 512, 50), (1024, 1024, 200), (2048, 2048, 800)] {
        let pixels = generate_test_image(width, height, num_stars);
        let background = create_background_map(width, height);
        let config = StarDetectionConfig::default();
        let size_name = format!("{}x{}_{}stars", width, height, num_stars);

        detect_group.throughput(Throughput::Elements((width * height) as u64));

        detect_group.bench_function(BenchmarkId::new("detect_stars", &size_name), |b| {
            b.iter(|| {
                black_box(detect_stars(
                    black_box(&pixels),
                    black_box(width),
                    black_box(height),
                    black_box(&background),
                    black_box(&config),
                ))
            })
        });
    }

    detect_group.finish();

    // detect_stars_filtered benchmarks
    let mut filtered_group = c.benchmark_group("detect_stars_filtered");
    filtered_group.sample_size(20);

    for &(width, height, num_stars) in &[(512, 512, 50), (1024, 1024, 200)] {
        let pixels = generate_test_image(width, height, num_stars);
        let filtered = generate_test_image(width, height, num_stars); // Simulated filtered image
        let background = create_background_map(width, height);
        let config = StarDetectionConfig::default();
        let size_name = format!("{}x{}_{}stars", width, height, num_stars);

        filtered_group.throughput(Throughput::Elements((width * height) as u64));

        filtered_group.bench_function(BenchmarkId::new("detect_stars_filtered", &size_name), |b| {
            b.iter(|| {
                black_box(detect_stars_filtered(
                    black_box(&pixels),
                    black_box(&filtered),
                    black_box(width),
                    black_box(height),
                    black_box(&background),
                    black_box(&config),
                ))
            })
        });
    }

    filtered_group.finish();
}
