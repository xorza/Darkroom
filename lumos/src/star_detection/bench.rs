//! Benchmark for the full star detection pipeline.
//!
//! Run with: cargo bench -p lumos --features bench --bench star_detection_full

use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

use crate::AstroImage;
use crate::astro_image::ImageDimensions;
use crate::star_detection::StarDetector;
use crate::testing::synthetic::stamps;

/// Register full pipeline benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("find_stars_full_pipeline");
    group.sample_size(10);

    // Test various image sizes and star densities
    for &(width, height, num_stars) in &[
        // (512, 512, 50),
        // (1024, 1024, 200),
        // (2048, 2048, 800),
        (6144, 6144, 3000),
    ] {
        let pixels = stamps::benchmark_star_field(width, height, num_stars, 0.1, 0.01, 12345);
        let image =
            AstroImage::from_pixels(ImageDimensions::new(width, height, 1), pixels.into_vec());

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
}
