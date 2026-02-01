//! Benchmarks for connected component labeling.

use super::LabelMap;
use crate::common::BitBuffer2;
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::star_detection::common::{dilate_mask, threshold_mask::create_threshold_mask};
use crate::testing::synthetic::stamps::benchmark_star_field;
use ::bench::quick_bench;
use std::hint::black_box;

/// Create a threshold mask using the real detection pipeline.
/// Uses background estimation, sigma thresholding, and dilation.
fn create_detection_mask(pixels: &crate::common::Buffer2<f32>, sigma_threshold: f32) -> BitBuffer2 {
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
