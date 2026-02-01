//! Benchmark module for background estimation.
//! Run with: cargo test -p lumos --release bench_background -- --ignored --nocapture

use bench::quick_bench;
use std::hint::black_box;

use super::tile_grid::TileGrid;
use crate::BackgroundConfig;
use crate::common::BitBuffer2;
use crate::star_detection::background::BackgroundMap;
use crate::testing::synthetic::{generate_globular_cluster, stamps};

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_background_estimate_6k(b: ::bench::Bencher) {
    let width = 6144;
    let height = 6144;
    let num_stars = (width * height) / 1000;

    let pixels = stamps::benchmark_star_field(width, height, num_stars, 0.1, 0.01, 42);
    let config = BackgroundConfig {
        tile_size: 64,
        ..Default::default()
    };

    b.bench(|| black_box(BackgroundMap::new(&pixels, &config)));
}

const BENCH_SIGMA_CLIP_ITERATIONS: usize = 2;

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_tile_grid_6k_globular(b: ::bench::Bencher) {
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);

    b.bench(|| {
        black_box(TileGrid::new_with_options(
            &pixels,
            64,
            None,
            0,
            BENCH_SIGMA_CLIP_ITERATIONS,
            None,
        ))
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

    b.bench(|| {
        black_box(TileGrid::new_with_options(
            &pixels,
            64,
            Some(&mask),
            100,
            BENCH_SIGMA_CLIP_ITERATIONS,
            None,
        ))
    });
}
