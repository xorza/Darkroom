//! Benchmark module for background estimation. Run with: cargo test -p lumos --release --features
//! bench `bench_background` -- --ignored --nocapture

use crate::testing::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::resources::DetectionResources;
use crate::testing::synthetic::background_map;
use crate::testing::synthetic::fixtures::star_field;

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_background_estimate_6k(b: ::quickbench::Bencher) {
    let width = 6144;
    let height = 6144;
    let num_stars = (width * height) / 1000;

    let pixels = star_field(Size2us::new(width, height), num_stars, 42)
        .image
        .channel(0)
        .clone();
    let config = BackgroundConfig {
        tile_size: 64,
        ..Default::default()
    };
    let mut resources = DetectionResources::new(Size2us::new(width, height));

    b.bench(|| {
        let bg = background_map::estimate_in(&pixels, &config, &mut resources);
        black_box(&bg);
        bg.release_to_pool(&mut resources);
    });
}
