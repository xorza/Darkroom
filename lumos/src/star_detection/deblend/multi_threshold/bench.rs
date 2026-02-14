//! Benchmarks for multi-threshold deblending.
//!
//! Run with: `cargo test -p lumos --release bench_multi_threshold -- --ignored --nocapture`

use ::bench::quick_bench;
use std::hint::black_box;

use super::{DeblendBuffers, deblend_multi_threshold};
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

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_deblend_multi_threshold_6k_dense(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    // Filter out huge components - multi-threshold is O(n * n_thresholds) per component
    // and not practical for >100k pixels
    let reasonable_components: Vec<_> = components.iter().filter(|c| c.area < 100_000).collect();

    let n_thresholds = 32;
    let min_separation = 3;
    let min_contrast = 0.005;

    let mut buffers = DeblendBuffers::new();

    b.bench(|| {
        for component in &reasonable_components {
            black_box(deblend_multi_threshold(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(n_thresholds),
                black_box(min_separation),
                black_box(min_contrast),
                &mut buffers,
            ));
        }
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_deblend_multi_threshold_6k_dense_fewer_levels(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    // Filter out huge components - multi-threshold is O(n^2) and not practical for >100k pixels
    let reasonable_components: Vec<_> = components
        .iter()
        .filter(|c| c.area < 100_000)
        .take(5000)
        .collect();

    let n_thresholds = 16;
    let min_separation = 3;
    let min_contrast = 0.005;

    let mut buffers = DeblendBuffers::new();

    b.bench(|| {
        for component in &reasonable_components {
            black_box(deblend_multi_threshold(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(n_thresholds),
                black_box(min_separation),
                black_box(min_contrast),
                &mut buffers,
            ));
        }
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_multi_threshold_4k_dense(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(4096, 4096, 20000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    // Filter out huge components - multi-threshold is O(n^2) and not practical for >100k pixels
    let reasonable_components: Vec<_> = components.iter().filter(|c| c.area < 100_000).collect();

    let n_thresholds = 32;
    let min_separation = 3;
    let min_contrast = 0.005;

    // Reuse buffers across components (same as real pipeline via rayon fold)
    let mut buffers = DeblendBuffers::new();

    b.bench(|| {
        for component in &reasonable_components {
            black_box(deblend_multi_threshold(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(n_thresholds),
                black_box(min_separation),
                black_box(min_contrast),
                &mut buffers,
            ));
        }
    });
}
