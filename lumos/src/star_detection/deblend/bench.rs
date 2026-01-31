//! Benchmarks for deblending algorithms.
//!
//! Run with: `cargo bench -p lumos --features bench --bench deblend`
//!
//! To generate the dense field image for inspection:
//! `cargo test -p lumos save_dense_field_image -- --nocapture`

use ::bench::quick_bench;
use std::hint::black_box;

use super::local_maxima::{deblend_local_maxima, find_local_maxima};
use super::multi_threshold::deblend_multi_threshold;
use super::{ComponentData, DeblendConfig};
use crate::common::{BitBuffer2, Buffer2};
use crate::math::{Aabb, Vec2us};
use crate::star_detection::detection::LabelMap;
use crate::testing::synthetic::generate_globular_cluster;

/// Create components from a pixel buffer for benchmarking.
///
/// Applies threshold, creates label map, and extracts component data.
fn create_components_from_pixels(
    pixels: &Buffer2<f32>,
    threshold: f32,
) -> (LabelMap, Vec<ComponentData>) {
    let width = pixels.width();
    let height = pixels.height();

    // Create threshold mask
    let mut mask = BitBuffer2::new_filled(width, height, false);
    for (idx, &value) in pixels.iter().enumerate() {
        if value > threshold {
            mask.set(idx, true);
        }
    }

    // Create label map
    let labels = LabelMap::from_mask(&mask);
    let num_labels = labels.num_labels();

    // Extract component data
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

/// Save a synthetic dense field image for visual inspection.
#[test]
#[ignore]
fn save_globular_cluster_image() {
    use image::GrayImage;

    let width = 6144;
    let height = 6144;
    let num_stars = 50000;

    let pixels = generate_globular_cluster(width, height, num_stars, 42);
    let (_labels, components) = create_components_from_pixels(&pixels, 0.05);

    // Find largest component (should be the core)
    let largest_component = components.iter().max_by_key(|c| c.area);
    let largest_area = largest_component.map(|c| c.area).unwrap_or(0);

    // Convert to u8 with asinh stretch for better dynamic range
    let max_val = pixels.iter().fold(f32::MIN, |a, &b| a.max(b));
    let stretch_factor = 5.0;

    let bytes: Vec<u8> = pixels
        .iter()
        .map(|&p| {
            let normalized = p / max_val;
            let stretched = (normalized * stretch_factor).asinh() / stretch_factor.asinh();
            (stretched.clamp(0.0, 1.0) * 255.0) as u8
        })
        .collect();

    let image = GrayImage::from_raw(width as u32, height as u32, bytes).unwrap();
    let output_path = common::test_utils::test_output_path("deblend_bench/globular_cluster_6k.jpg");
    image.save(&output_path).unwrap();

    println!("Saved dense field image to: {:?}", output_path);
    println!("Image size: {}x{}", width, height);
    println!("Number of stars: {}", num_stars);
    println!("Number of components: {}", components.len());
    println!("Largest component area: {} pixels", largest_area);
}

// ============================================================================
// Local maxima benchmarks
// ============================================================================

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_find_local_maxima_6k_dense(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);
    let config = DeblendConfig::default();

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
                black_box(&config),
            ));
        }
    });
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_deblend_local_maxima_6k_dense(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);
    let config = DeblendConfig::default();

    b.bench(|| {
        for component in &components {
            black_box(deblend_local_maxima(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(&config),
            ));
        }
    });
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_deblend_local_maxima_6k_dense_tight_separation(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);
    // Crowded field config: tighter separation, lower prominence threshold
    let config = DeblendConfig {
        min_separation: 2,
        min_prominence: 0.2,
        ..Default::default()
    };

    println!("Using config: {:?}", components.len());

    b.bench(|| {
        for component in &components {
            black_box(deblend_local_maxima(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(&config),
            ));
        }
    });
}

// ============================================================================
// Multi-threshold benchmarks
// ============================================================================

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_deblend_multi_threshold_6k_dense(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    // // Filter out huge components - multi-threshold is O(n²) and not practical for >100k pixels
    // let reasonable_components: Vec<_> = components.iter().filter(|c| c.area < 100_000).collect();

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    b.bench(|| {
        for component in &components {
            black_box(deblend_multi_threshold(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(&config),
            ));
        }
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_deblend_multi_threshold_6k_dense_fewer_levels(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    // Filter out huge components - multi-threshold is O(n²) and not practical for >100k pixels
    let reasonable_components: Vec<_> = components.iter().filter(|c| c.area < 100_000).collect();

    let config = DeblendConfig {
        n_thresholds: 16,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    b.bench(|| {
        for component in &reasonable_components {
            black_box(deblend_multi_threshold(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(&config),
            ));
        }
    });
}

// ============================================================================
// Comparison benchmarks (smaller images for quick comparison)
// ============================================================================

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_local_vs_multi_4k_dense_local(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(4096, 4096, 20000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);
    let config = DeblendConfig::default();

    b.bench(|| {
        for component in &components {
            black_box(deblend_local_maxima(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(&config),
            ));
        }
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_local_vs_multi_4k_dense_multi(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(4096, 4096, 20000, 42);
    let (labels, components) = create_components_from_pixels(&pixels, 0.05);

    // Filter out huge components - multi-threshold is O(n²) and not practical for >100k pixels
    let reasonable_components: Vec<_> = components.iter().filter(|c| c.area < 100_000).collect();

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    b.bench(|| {
        for component in &reasonable_components {
            black_box(deblend_multi_threshold(
                black_box(component),
                black_box(&pixels),
                black_box(&labels),
                black_box(&config),
            ));
        }
    });
}
