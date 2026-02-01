//! Benchmarks for full star detection pipeline.
//!
//! Run with: `cargo test -p lumos --release bench_star_detection -- --ignored --nocapture`

use ::bench::quick_bench;
use std::hint::black_box;

use crate::astro_image::ImageDimensions;
use crate::testing::init_tracing;
use crate::testing::synthetic::generate_globular_cluster;
use crate::{AstroImage, StarDetectionConfig, StarDetector};

#[quick_bench(warmup_iters = 3, iters = 30)]
fn bench_detect_6k_globular_cluster(b: ::bench::Bencher) {
    init_tracing();

    // 6K globular cluster with 50000 stars - extreme crowding
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let image = AstroImage::from_pixels(
        ImageDimensions::new(pixels.width(), pixels.height(), 1),
        pixels.into_vec(),
    );
    let config = StarDetectionConfig::for_crowded_field();
    let detector = StarDetector::from_config(config);

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_detect_4k_dense(b: ::bench::Bencher) {
    use crate::testing::synthetic::stamps::benchmark_star_field;

    // 4K image with 2000 stars
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let image = AstroImage::from_pixels(
        ImageDimensions::new(pixels.width(), pixels.height(), 1),
        pixels.into_vec(),
    );
    let detector = StarDetector::new();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_detect_1k_sparse(b: ::bench::Bencher) {
    use crate::testing::synthetic::stamps::benchmark_star_field;

    // 1K image with 100 stars (sparse field)
    let pixels = benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 42);
    let image = AstroImage::from_pixels(
        ImageDimensions::new(pixels.width(), pixels.height(), 1),
        pixels.into_vec(),
    );
    let detector = StarDetector::new();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}
