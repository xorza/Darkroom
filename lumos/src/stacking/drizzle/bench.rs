//! Benchmarks for drizzle reconstruction (Fruchter & Hook) across kernels, on synthetic
//! dithered frames. The expensive part is the per-frame flux distribution, which differs by
//! kernel — Turbo (axis-aligned) vs Square (exact polygon clipping) vs Gaussian (droplet).
//!
//! Run: `cargo test -p lumos --release drizzle::bench -- --ignored --nocapture`

use glam::DVec2;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::io::image::LinearImage;
use crate::stacking::drizzle::accumulator::DrizzleFrame;
use crate::stacking::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::stacking::drizzle::stack::drizzle_images;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::transform::Transform;
use crate::testing::synthetic::fixtures::star_field;

const N_FRAMES: usize = 8;

/// `N_FRAMES` copies of one synthetic field, each with a small sub-pixel dither — the input a
/// drizzle integration sees.
fn dithered_set() -> Vec<DrizzleFrame<LinearImage>> {
    let base = star_field(1000, 1000, 250, 5).image;
    (0..N_FRAMES)
        .map(|i| {
            let dx = (i as f64 * 0.37).fract() * 2.0 - 1.0;
            let dy = (i as f64 * 0.71).fract() * 2.0 - 1.0;
            DrizzleFrame::new(base.clone(), Transform::translation(DVec2::new(dx, dy)))
        })
        .collect()
}

fn bench_kernel(b: ::quickbench::Bencher, kernel: DrizzleKernel) {
    let frames = dithered_set();
    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel,
        ..DrizzleConfig::default()
    };
    b.bench(|| {
        black_box(drizzle_images(
            frames.clone(),
            &config,
            ProgressCallback::default(),
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_drizzle_turbo_8(b: ::quickbench::Bencher) {
    bench_kernel(b, DrizzleKernel::Turbo);
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_drizzle_square_8(b: ::quickbench::Bencher) {
    bench_kernel(b, DrizzleKernel::Square);
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_drizzle_gaussian_8(b: ::quickbench::Bencher) {
    bench_kernel(b, DrizzleKernel::Gaussian);
}
