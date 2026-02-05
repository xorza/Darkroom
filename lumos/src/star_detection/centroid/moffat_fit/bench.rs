//! Benchmarks for Moffat fitting.
//!
//! Run with: `cargo test -p lumos --release bench_moffat -- --ignored --nocapture`

use bench::quick_bench;
use std::hint::black_box;

use super::{MoffatFitConfig, fit_moffat_2d};
use crate::common::Buffer2;
use glam::Vec2;

/// Create a synthetic Moffat stamp for benchmarking.
fn make_moffat_stamp(width: usize, height: usize, cx: f32, cy: f32) -> Buffer2<f32> {
    let alpha = 2.5f32;
    let beta = 2.5f32;
    let amp = 1.0f32;
    let bg = 0.1f32;
    let alpha2 = alpha * alpha;
    let mut pixels = vec![bg; width * height];
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
            pixels[y * width + x] += amp * (1.0 + r2 / alpha2).powf(-beta);
        }
    }
    Buffer2::new(width, height, pixels)
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_moffat_fit_fixed_beta_small(b: bench::Bencher) {
    // 17x17 stamp
    let pixels = make_moffat_stamp(17, 17, 8.3, 8.7);
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
    let pixels = make_moffat_stamp(25, 25, 12.3, 12.7);
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
    let pixels = make_moffat_stamp(21, 21, 10.3, 10.7);
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
