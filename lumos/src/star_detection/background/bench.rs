//! Benchmark module for background estimation.
//! Run with: cargo bench --package lumos --features bench background

use bench::quick_bench;

use crate::testing::synthetic::stamps;
use crate::{BackgroundConfig, estimate_background_iterative};

use std::hint::black_box;

#[quick_bench(warmup_iters = 2, iters = 5)]
fn estimate_background_iterative_6k(b: ::bench::Bencher) {
    let width = 6144;
    let height = 6144;
    let num_stars = (width * height) / 1000;

    let pixels = stamps::benchmark_star_field(width, height, num_stars, 0.1, 0.01, 42);
    let iter_config = BackgroundConfig::default();

    b.bench(|| black_box(estimate_background_iterative(&pixels, 64, &iter_config)));
}
