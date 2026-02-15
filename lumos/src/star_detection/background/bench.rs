//! Benchmark module for background estimation.
//! Run with: cargo test -p lumos --release bench_background -- --ignored --nocapture

use bench::quick_bench;
use std::hint::black_box;

use super::tile_grid::TileGrid;
use super::{BufferPool, estimate_background, refine_background};
use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::background::BackgroundEstimate;
use crate::star_detection::config::Config;
use crate::testing::synthetic::{generate_globular_cluster, stamps};

/// Estimate background with automatic buffer pool management (bench helper).
fn estimate_background_test(pixels: &Buffer2<f32>, config: &Config) -> BackgroundEstimate {
    let mut pool = BufferPool::new(pixels.width(), pixels.height());
    let mut estimate = estimate_background(pixels, config, &mut pool);
    if config.refinement.iterations() > 0 {
        refine_background(pixels, &mut estimate, config, &mut pool);
    }
    estimate
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_background_estimate_6k(b: ::bench::Bencher) {
    let width = 6144;
    let height = 6144;
    let num_stars = (width * height) / 1000;

    let pixels = stamps::benchmark_star_field(width, height, num_stars, 0.1, 0.01, 42);
    let config = Config {
        tile_size: 64,
        ..Default::default()
    };

    b.bench(|| {
        let bg = estimate_background_test(&pixels, &config);
        black_box(&bg);
    });
}

const BENCH_SIGMA_CLIP_ITERATIONS: usize = 2;

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_tile_grid_6k_globular(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let mut grid = TileGrid::new_uninit(pixels.width(), pixels.height(), 64);

    b.bench(|| {
        grid.compute(&pixels, None, BENCH_SIGMA_CLIP_ITERATIONS);
        black_box(&grid);
    });
}

#[quick_bench(warmup_iters = 2, iters = 50)]
fn bench_tile_grid_6k_with_mask(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);

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

    let mut grid = TileGrid::new_uninit(width, height, 64);

    b.bench(|| {
        grid.compute(&pixels, Some(&mask), BENCH_SIGMA_CLIP_ITERATIONS);
        black_box(&grid);
    });
}
