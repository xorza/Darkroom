//! Benchmark for the full star detection pipeline.
//!
//! Run with: cargo bench -p lumos --features bench --bench star_detection_full

use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

use crate::AstroImage;
use crate::astro_image::ImageDimensions;
use crate::star_detection::StarDetector;

/// Generate a synthetic star field image for benchmarking.
fn generate_synthetic_image(width: usize, height: usize, num_stars: usize) -> AstroImage {
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

        let cx = 20 + (hash1 % (width - 40));
        let cy = 20 + (hash2 % (height - 40));
        let brightness = 0.3 + (hash3 % 700) as f32 / 1000.0; // 0.3 to 1.0
        let sigma = 1.5 + (hash4 % 150) as f32 / 100.0; // 1.5 to 3.0 pixels FWHM ~3.5-7

        for dy in -10i32..=10 {
            for dx in -10i32..=10 {
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

    // Clamp to valid range
    for p in &mut pixels {
        *p = p.clamp(0.0, 1.0);
    }

    AstroImage::from_pixels(ImageDimensions::new(width, height, 1), pixels)
}

/// Register full pipeline benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("find_stars_full_pipeline");
    group.sample_size(200);

    // Test various image sizes and star densities
    for &(width, height, num_stars) in &[
        // (512, 512, 50),
        // (1024, 1024, 200),
        // (2048, 2048, 800),
        (6144, 6144, 3000),
    ] {
        let image = generate_synthetic_image(width, height, num_stars);

        let size_name = format!("{}x{}_{}stars", width, height, num_stars);

        group.throughput(Throughput::Elements((width * height) as u64));

        // Default configuration (no matched filter)
        let detector_default = StarDetector::new();
        group.bench_function(BenchmarkId::new("default", &size_name), |b| {
            b.iter(|| black_box(detector_default.detect(black_box(&image))))
        });

        // With matched filter (FWHM = 4.0)
        let detector_matched = StarDetector::new().with_fwhm(4.0);
        group.bench_function(BenchmarkId::new("matched_filter", &size_name), |b| {
            b.iter(|| black_box(detector_matched.detect(black_box(&image))))
        });
    }

    group.finish();

    // Benchmark different configurations
    // let mut config_group = c.benchmark_group("find_stars_configurations");
    // config_group.sample_size(20);

    // let image = generate_synthetic_image(2048, 2048, 800);

    // // Default
    // let detector = StarDetector::new();
    // config_group.bench_function("default_2048x2048", |b| {
    //     b.iter(|| black_box(detector.detect(black_box(&image))))
    // });

    // // Wide field preset
    // let detector = StarDetector::new().for_wide_field();
    // config_group.bench_function("wide_field_2048x2048", |b| {
    //     b.iter(|| black_box(detector.detect(black_box(&image))))
    // });

    // // High resolution preset
    // let detector = StarDetector::new().for_high_resolution();
    // config_group.bench_function("high_resolution_2048x2048", |b| {
    //     b.iter(|| black_box(detector.detect(black_box(&image))))
    // });

    // // Crowded field preset
    // let detector = StarDetector::new().for_crowded_field();
    // config_group.bench_function("crowded_field_2048x2048", |b| {
    //     b.iter(|| black_box(detector.detect(black_box(&image))))
    // });

    // // With matched filter (more expensive)
    // let detector = StarDetector::new().with_fwhm(4.0);
    // config_group.bench_function("matched_filter_2048x2048", |b| {
    //     b.iter(|| black_box(detector.detect(black_box(&image))))
    // });

    // config_group.finish();
}
