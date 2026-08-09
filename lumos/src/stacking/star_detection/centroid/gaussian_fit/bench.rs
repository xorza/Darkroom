//! Benchmarks for Gaussian fitting.
//!
//! Run with: `cargo test -p lumos --release bench_gaussian -- --ignored --nocapture`
use crate::stacking::star_detection::centroid::StampGrid;
use crate::testing::prelude::*;

use quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::star_detection::centroid::gaussian_fit::GaussianFit;
use crate::stacking::star_detection::centroid::gaussian_fit::GaussianFitConfig;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_gaussian_fit_small(b: quickbench::Bencher) {
    // 17x17 stamp
    let pixels = SyntheticStar::new(
        Vec2::new(8.3, 8.7),
        1.0,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(17, 17), 0.1);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(GaussianFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(8.0)),
            black_box(&StampGrid::new(8)),
            black_box(0.1),
            None,
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_gaussian_fit_medium(b: quickbench::Bencher) {
    // 25x25 stamp
    let pixels = SyntheticStar::new(
        Vec2::new(12.3, 12.7),
        1.0,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(25, 25), 0.1);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(GaussianFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(12.0)),
            black_box(&StampGrid::new(12)),
            black_box(0.1),
            None,
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_gaussian_fit_large(b: quickbench::Bencher) {
    // 31x31 stamp
    let pixels = SyntheticStar::new(
        Vec2::new(15.3, 15.7),
        1.0,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(31, 31), 0.1);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(GaussianFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(15.0)),
            black_box(&StampGrid::new(15)),
            black_box(0.1),
            None,
            black_box(&config),
        ))
    });
}
