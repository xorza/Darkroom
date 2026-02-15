//! Benchmarks for local maxima deblending.
//!
//! Run with: `cargo test -p lumos --release bench_local_maxima -- --ignored --nocapture`

use ::bench::quick_bench;
use std::hint::black_box;

use super::{deblend_local_maxima, find_local_maxima};
use crate::common::{BitBuffer2, Buffer2};
use crate::math::{Aabb, Vec2us};
use crate::star_detection::config::Connectivity;
use crate::star_detection::deblend::ComponentData;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::labeling::test_utils::label_map_from_mask_with_connectivity;
use crate::testing::synthetic::generate_globular_cluster;

/// Create components from a pixel buffer for benchmarking.
fn create_components_from_pixels(
    pixels: &Buffer2<f32>,
    threshold: f32,
) -> (LabelMap, Vec<ComponentData>) {
    let width = pixels.width();
    let height = pixels.height();

    let mut mask = BitBuffer2::new_filled(width, height, false);
    for (idx, &value) in pixels.iter().enumerate() {
        if value > threshold {
            mask.set(idx, true);
        }
    }

    let labels = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    let num_labels = labels.num_labels();

    let mut components = Vec::with_capacity(num_labels);
    let mut bboxes = vec![Aabb::empty(); num_labels + 1];
    let mut areas = vec![0usize; num_labels + 1];

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let label = labels[idx];
            if label > 0 {
                bboxes[label as usize].include(Vec2us::new(x, y));
                areas[label as usize] += 1;
            }
        }
    }

    for label in 1..=num_labels {
        if areas[label] > 0 {
            components.push(ComponentData {
                bbox: bboxes[label],
                label: label as u32,
                area: areas[label],
            });
        }
    }

    (labels, components)
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_find_local_maxima_6k_dense(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    // Find the 100 largest components for benchmarking
    let mut sorted_components = components.clone();
    sorted_components.sort_by_key(|c| std::cmp::Reverse(c.area));
    let large_components: Vec<_> = sorted_components.into_iter().take(100).collect();

    b.bench(|| {
        for component in &large_components {
            black_box(find_local_maxima(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(3),
                black_box(0.3),
            ));
        }
    });
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_deblend_local_maxima_6k_dense(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    b.bench(|| {
        for component in &components {
            black_box(deblend_local_maxima(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(3),
                black_box(0.3),
            ));
        }
    });
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_local_maxima_4k_dense(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(4096, 4096, 20000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    b.bench(|| {
        for component in &components {
            black_box(deblend_local_maxima(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(3),
                black_box(0.3),
            ));
        }
    });
}
