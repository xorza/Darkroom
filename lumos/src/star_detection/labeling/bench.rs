//! Benchmarks for connected component labeling.

use crate::bit_buffer2::BitBuffer2;
use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::internals::synthetic::fixtures::star_field;
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::config::detection_config::Connectivity;
use crate::star_detection::labeling::labeler::Labeler;
use crate::star_detection::threshold_mask::{ThresholdParams, create_residual_threshold_mask};
use ::quickbench::quick_bench;
use std::hint::black_box;

/// The mask the detect stage labels, without its matched filter: the residual of `pixels` past
/// `sigma_threshold` of its sky noise.
fn create_detection_mask(pixels: &Buffer2<f32>, sigma_threshold: f32) -> BitBuffer2 {
    let background = background_map::estimate(pixels, &BackgroundConfig::default());
    let sky = background.sky_noise();
    let mut mask = BitBuffer2::new_filled(Size2us::new(pixels.width(), pixels.height()), false);
    create_residual_threshold_mask(
        &background.residual_of(pixels),
        &sky.noise,
        ThresholdParams {
            sigma: sigma_threshold,
            min_noise: sky.floor,
        },
        &mut mask,
    );
    mask
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_label_map_from_buffer_1k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(1024, 1024), 500, 42)
        .image
        .channel(0)
        .clone();
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labeler = Labeler::default();
    let mut labels = Some(Buffer2::new_filled(1024, 1024, 0u32));

    b.bench(|| {
        let mut buffer = labels.take().expect("the last iteration returned it");
        buffer.pixels_mut().fill(0);
        let map = labeler.label(black_box(&mask), Connectivity::Four, buffer);
        black_box(map.num_labels());
        labels = Some(map.labels);
        labeler.recycle(map.components);
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_label_map_from_buffer_4k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(4096, 4096), 2000, 42)
        .image
        .channel(0)
        .clone();
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labeler = Labeler::default();
    let mut labels = Some(Buffer2::new_filled(4096, 4096, 0u32));

    b.bench(|| {
        let mut buffer = labels.take().expect("the last iteration returned it");
        buffer.pixels_mut().fill(0);
        let map = labeler.label(black_box(&mask), Connectivity::Four, buffer);
        black_box(map.num_labels());
        labels = Some(map.labels);
        labeler.recycle(map.components);
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_label_map_from_buffer_4k_globular(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(4096, 4096), 50000, 42)
        .image
        .channel(0)
        .clone();
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labeler = Labeler::default();
    let mut labels = Some(Buffer2::new_filled(4096, 4096, 0u32));

    b.bench(|| {
        let mut buffer = labels.take().expect("the last iteration returned it");
        buffer.pixels_mut().fill(0);
        let map = labeler.label(black_box(&mask), Connectivity::Four, buffer);
        black_box(map.num_labels());
        labels = Some(map.labels);
        labeler.recycle(map.components);
    });
}

/// The labeler on an image small enough to resolve to a single strip, where the boundary stitch
/// has nothing to do — the single-strip path, where the parallel labeler carries no stitch cost.
#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_label_small(b: ::quickbench::Bencher) {
    let size = Size2us::new(240, 240);
    let pixels = star_field(size, 30, 42).image.channel(0).clone();
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labeler = Labeler::default();
    let mut labels = Some(Buffer2::new_filled(size.width, size.height, 0u32));

    b.bench(|| {
        let mut buffer = labels.take().expect("the last iteration returned it");
        buffer.pixels_mut().fill(0);
        let map = labeler.label(black_box(&mask), Connectivity::Four, buffer);
        black_box(map.num_labels());
        labels = Some(map.labels);
        labeler.recycle(map.components);
    });
}
