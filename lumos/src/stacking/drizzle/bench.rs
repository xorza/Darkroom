//! Benchmarks for drizzle reconstruction (Fruchter & Hook) across kernels, on synthetic
//! dithered frames. The expensive part is the per-frame flux distribution, which differs by
//! kernel — Turbo (axis-aligned) vs Square (exact polygon clipping) vs Gaussian (droplet).
//!
//! Run: `cargo test -p lumos --release drizzle::bench -- --ignored --nocapture`

use glam::DVec2;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::testing::synthetic::fixtures::star_field;
use crate::{
    AstroImage, DrizzleConfig, DrizzleKernel, ProgressCallback, Transform, drizzle_images,
};

const N_FRAMES: usize = 8;

/// `N_FRAMES` copies of one synthetic field, each with a small sub-pixel dither — the input a
/// drizzle integration sees.
fn dithered_set() -> (Vec<AstroImage>, Vec<Transform>) {
    let base = star_field(1000, 1000, 250, 5).image;
    let mut images = Vec::with_capacity(N_FRAMES);
    let mut transforms = Vec::with_capacity(N_FRAMES);
    for i in 0..N_FRAMES {
        images.push(base.clone());
        let dx = (i as f64 * 0.37).fract() * 2.0 - 1.0;
        let dy = (i as f64 * 0.71).fract() * 2.0 - 1.0;
        transforms.push(Transform::translation(DVec2::new(dx, dy)));
    }
    (images, transforms)
}

fn bench_kernel(b: ::quickbench::Bencher, kernel: DrizzleKernel) {
    let (images, transforms) = dithered_set();
    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel,
        ..DrizzleConfig::default()
    };
    b.bench(|| {
        black_box(drizzle_images(
            images.clone(),
            &transforms,
            None,
            None,
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
