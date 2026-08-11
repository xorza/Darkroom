//! Benchmarks for Moffat fitting.
//!
//! Run with: `cargo test -p lumos --release bench_moffat -- --ignored --nocapture`
use crate::stacking::star_detection::centroid::stamp::StampGrid;
use crate::testing::prelude::*;

use quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::star_detection::centroid::moffat_fit::{MoffatFit, MoffatFitConfig};
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_moffat_fit_fixed_beta_small(b: quickbench::Bencher) {
    // 17x17 stamp
    let pixels = SyntheticStar::new(
        Vec2::new(8.3, 8.7),
        1.0,
        StarProfile::Moffat {
            alpha: 2.5,
            beta: 2.5,
        },
    )
    .stamp(Size2us::new(17, 17), 0.1);
    let config = MoffatFitConfig {
        fixed_beta: 2.5,
        ..Default::default()
    };

    b.bench(|| {
        black_box(MoffatFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(8.0)),
            black_box(&StampGrid::new(8)),
            black_box(0.1),
            None,
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_moffat_fit_fixed_beta_medium(b: quickbench::Bencher) {
    // 25x25 stamp
    let pixels = SyntheticStar::new(
        Vec2::new(12.3, 12.7),
        1.0,
        StarProfile::Moffat {
            alpha: 2.5,
            beta: 2.5,
        },
    )
    .stamp(Size2us::new(25, 25), 0.1);
    let config = MoffatFitConfig {
        fixed_beta: 2.5,
        ..Default::default()
    };

    b.bench(|| {
        black_box(MoffatFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(12.0)),
            black_box(&StampGrid::new(12)),
            black_box(0.1),
            None,
            black_box(&config),
        ))
    });
}
