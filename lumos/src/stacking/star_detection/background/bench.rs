//! Benchmark module for background estimation.
//! Run with: cargo test -p lumos --release bench_background -- --ignored --nocapture

use quickbench::quick_bench;
use std::hint::black_box;

use crate::background_mesh::workspace::MeshWorkspace;
use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use crate::stacking::star_detection::background::{estimate_background, refine_background};
use crate::stacking::star_detection::config::BackgroundConfig;
use crate::stacking::star_detection::resources::DetectionResources;
use crate::testing::synthetic::fixtures::{cluster_field, star_field};
use common::BitBuffer2;
use imaginarium::Buffer2;

/// Estimate background with automatic buffer pool management (bench helper).
fn estimate_background_test(
    pixels: &Buffer2<f32>,
    config: &BackgroundConfig,
    resources: &mut DetectionResources,
) -> BackgroundEstimate {
    let mut estimate = estimate_background(pixels, config, resources);
    if config.refinement.iterations() > 0 {
        refine_background(pixels, &mut estimate, config, 4.0, resources);
    }
    estimate
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_background_estimate_6k(b: ::quickbench::Bencher) {
    let width = 6144;
    let height = 6144;
    let num_stars = (width * height) / 1000;

    let pixels = star_field(width, height, num_stars, 42)
        .image
        .channel(0)
        .clone();
    let config = BackgroundConfig {
        tile_size: 64,
        ..Default::default()
    };
    let mut resources = DetectionResources::new(width, height);

    b.bench(|| {
        let bg = estimate_background_test(&pixels, &config, &mut resources);
        black_box(&bg);
        bg.release_to_pool(&mut resources);
    });
}

const BENCH_SIGMA_CLIP_ITERATIONS: usize = 2;

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_tile_grid_6k_globular(b: ::quickbench::Bencher) {
    let pixels = cluster_field(6144, 6144, 50000, 42)
        .image
        .channel(0)
        .clone();
    let mut workspace = MeshWorkspace::default();

    b.bench(|| {
        let grid = workspace.compute(&pixels, None, 64, BENCH_SIGMA_CLIP_ITERATIONS, true);
        black_box(grid);
    });
}

#[quick_bench(warmup_iters = 2, iters = 50)]
fn bench_tile_grid_6k_with_mask(b: ::quickbench::Bencher) {
    let pixels = cluster_field(6144, 6144, 50000, 42)
        .image
        .channel(0)
        .clone();

    // Create mask from actual bright pixels (threshold at 0.1)
    let width = pixels.width();
    let height = pixels.height();
    let mut mask = BitBuffer2::new_filled(width, height, false);

    for (idx, &val) in pixels.iter().enumerate() {
        if val > 0.1 {
            mask.set(idx, true);
        }
    }

    let masked_count: usize = mask.words().iter().map(|w| w.count_ones() as usize).sum();
    println!(
        "Mask: {} pixels masked ({:.1}%)",
        masked_count,
        100.0 * masked_count as f64 / (width * height) as f64
    );

    let mut workspace = MeshWorkspace::default();

    b.bench(|| {
        let grid = workspace.compute(&pixels, Some(&mask), 64, BENCH_SIGMA_CLIP_ITERATIONS, true);
        black_box(grid);
    });
}
