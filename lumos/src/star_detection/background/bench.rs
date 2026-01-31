//! Benchmark module for background estimation.
//! Run with: cargo test -p lumos --release bench_background -- --ignored --nocapture

use bench::quick_bench;

use crate::BackgroundConfig;
use crate::star_detection::background::BackgroundMap;
use crate::testing::synthetic::stamps;

use std::hint::black_box;

#[quick_bench(warmup_iters = 2, iters = 5)]
fn background_estimate_6k(b: ::bench::Bencher) {
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
