//! Benchmarks for Gaussian fitting.
//!
//! Run with: `cargo test -p lumos --release bench_gaussian -- --ignored --nocapture`

use bench::quick_bench;
use std::hint::black_box;

use super::GaussianFitConfig;
use super::fit_gaussian_2d;
use crate::star_detection::centroid::test_utils::make_gaussian_star;
use glam::Vec2;

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_gaussian_fit_small(b: bench::Bencher) {
    // 17x17 stamp
    let pixels = make_gaussian_star(17, 17, Vec2::new(8.3, 8.7), 2.5, 1.0, 0.1);
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

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_gaussian_fit_medium(b: bench::Bencher) {
    // 25x25 stamp
    let pixels = make_gaussian_star(25, 25, Vec2::new(12.3, 12.7), 2.5, 1.0, 0.1);
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

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_gaussian_fit_large(b: bench::Bencher) {
    // 31x31 stamp
    let pixels = make_gaussian_star(31, 31, Vec2::new(15.3, 15.7), 2.5, 1.0, 0.1);
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
