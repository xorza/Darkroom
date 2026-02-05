//! Benchmarks for Gaussian fitting.
//!
//! Run with: `cargo test -p lumos --release bench_gaussian -- --ignored --nocapture`

use bench::quick_bench;
use std::hint::black_box;

use super::GaussianFitConfig;
use super::fit_gaussian_2d;
use crate::common::Buffer2;
use glam::Vec2;

/// Create a synthetic Gaussian stamp for benchmarking.
fn make_gaussian_stamp(width: usize, height: usize, cx: f32, cy: f32) -> Buffer2<f32> {
    let sigma = 2.5f32;
    let amp = 1.0f32;
    let bg = 0.1f32;
    let sigma2 = sigma * sigma;
    let mut pixels = vec![bg; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            pixels[y * width + x] += amp * (-0.5 * (dx * dx + dy * dy) / sigma2).exp();
        }
    }
    Buffer2::new(width, height, pixels)
}

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_gaussian_fit_small(b: bench::Bencher) {
    // 17x17 stamp
    let pixels = make_gaussian_stamp(17, 17, 8.3, 8.7);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(fit_gaussian_2d(
            black_box(&pixels),
            black_box(Vec2::splat(8.0)),
            black_box(8),
            black_box(0.1),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_gaussian_fit_medium(b: bench::Bencher) {
    // 25x25 stamp
    let pixels = make_gaussian_stamp(25, 25, 12.3, 12.7);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(fit_gaussian_2d(
            black_box(&pixels),
            black_box(Vec2::splat(12.0)),
            black_box(12),
            black_box(0.1),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_gaussian_fit_large(b: bench::Bencher) {
    // 31x31 stamp
    let pixels = make_gaussian_stamp(31, 31, 15.3, 15.7);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(fit_gaussian_2d(
            black_box(&pixels),
            black_box(Vec2::splat(15.0)),
            black_box(15),
            black_box(0.1),
            black_box(&config),
        ))
    });
}
