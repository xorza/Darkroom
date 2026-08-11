//! Benchmarks for connected component labeling.

use crate::bit_buffer2::BitBuffer2;
use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::config::detection_config::Connectivity;
use crate::stacking::star_detection::mask_dilation::dilate_mask;
use crate::stacking::star_detection::threshold_mask::create_threshold_mask;
use crate::testing::prelude::*;
use crate::testing::synthetic::fixtures::star_field;
use ::quickbench::quick_bench;
use std::hint::black_box;

/// Create a threshold mask using the real detection pipeline.
/// Uses background estimation, sigma thresholding, and dilation.
fn create_detection_mask(pixels: &Buffer2<f32>, sigma_threshold: f32) -> BitBuffer2 {
    let width = pixels.width();
    let height = pixels.height();

    // Create background map (same as real pipeline)
    let background = background_map::estimate(pixels, &BackgroundConfig::default());

    // Create threshold mask
    let mut mask = BitBuffer2::new_filled(Size2us::new(width, height), false);
    create_threshold_mask(
        pixels,
        &background.background,
        &background.noise,
        sigma_threshold,
        background.noise_floor,
        &mut mask,
    );

    // Dilate mask (same as real pipeline - radius 1)
    let mut dilated = BitBuffer2::new_filled(Size2us::new(width, height), false);
    dilate_mask(&mask, 1, &mut dilated);

    dilated
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_label_map_from_buffer_1k(b: ::quickbench::Bencher) {
    use crate::stacking::star_detection::labeling::label_mask;

    let pixels = star_field(Size2us::new(1024, 1024), 500, 42)
        .image
        .channel(0)
        .clone();
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labels = Buffer2::new_filled(1024, 1024, 0u32);

    b.bench(|| {
        labels.pixels_mut().fill(0);
        black_box(label_mask(
            black_box(&mask),
            &mut labels,
            Connectivity::Four,
        ))
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_label_map_from_buffer_4k(b: ::quickbench::Bencher) {
    use crate::stacking::star_detection::labeling::label_mask;

    let pixels = star_field(Size2us::new(4096, 4096), 2000, 42)
        .image
        .channel(0)
        .clone();
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labels = Buffer2::new_filled(4096, 4096, 0u32);

    b.bench(|| {
        labels.pixels_mut().fill(0);
        black_box(label_mask(
            black_box(&mask),
            &mut labels,
            Connectivity::Four,
        ))
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_label_map_from_buffer_4k_globular(b: ::quickbench::Bencher) {
    use crate::stacking::star_detection::labeling::label_mask;

    let pixels = star_field(Size2us::new(4096, 4096), 50000, 42)
        .image
        .channel(0)
        .clone();
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labels = Buffer2::new_filled(4096, 4096, 0u32);

    b.bench(|| {
        labels.pixels_mut().fill(0);
        black_box(label_mask(
            black_box(&mask),
            &mut labels,
            Connectivity::Four,
        ))
    });
}

/// The labeler on an image small enough to resolve to a single strip, where the boundary stitch
/// has nothing to do. Guards the path a second, sequential implementation used to serve.
#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_label_small(b: ::quickbench::Bencher) {
    use crate::stacking::star_detection::labeling::label_mask;

    let size = Size2us::new(240, 240);
    let pixels = star_field(size, 30, 42).image.channel(0).clone();
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labels = Buffer2::new_filled(size.width, size.height, 0u32);

    b.bench(|| {
        labels.pixels_mut().fill(0);
        black_box(label_mask(
            black_box(&mask),
            &mut labels,
            Connectivity::Four,
        ))
    });
}
