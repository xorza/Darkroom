//! Benchmarks for Moffat fitting.
//!
//! Run with: `cargo test -p lumos --release bench_moffat -- --ignored --nocapture`

use bench::quick_bench;
use std::hint::black_box;

use super::{MoffatFitConfig, fit_moffat_2d};
use crate::star_detection::centroid::test_utils::make_moffat_star;
use glam::Vec2;

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_moffat_fit_fixed_beta_small(b: bench::Bencher) {
    // 17x17 stamp
    let pixels = make_moffat_star(17, 17, Vec2::new(8.3, 8.7), 2.5, 2.5, 1.0, 0.1);
    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: 2.5,
        ..Default::default()
    };

    b.bench(|| {
        black_box(fit_moffat_2d(
            black_box(&pixels),
            black_box(Vec2::splat(8.0)),
            black_box(8),
            black_box(0.1),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_moffat_fit_fixed_beta_medium(b: bench::Bencher) {
    // 25x25 stamp
    let pixels = make_moffat_star(25, 25, Vec2::new(12.3, 12.7), 2.5, 2.5, 1.0, 0.1);
    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: 2.5,
        ..Default::default()
    };

    b.bench(|| {
        black_box(fit_moffat_2d(
            black_box(&pixels),
            black_box(Vec2::splat(12.0)),
            black_box(12),
            black_box(0.1),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_moffat_fit_variable_beta(b: bench::Bencher) {
    // 21x21 stamp with variable beta
    let pixels = make_moffat_star(21, 21, Vec2::new(10.3, 10.7), 2.5, 2.5, 1.0, 0.1);
    let config = MoffatFitConfig {
        fit_beta: true,
        fixed_beta: 2.5,
        ..Default::default()
    };

    b.bench(|| {
        black_box(fit_moffat_2d(
            black_box(&pixels),
            black_box(Vec2::splat(10.0)),
            black_box(8),
            black_box(0.1),
            black_box(&config),
        ))
    });
}
