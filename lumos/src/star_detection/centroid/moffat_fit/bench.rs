//! Benchmarks for Moffat fitting.
//!
//! Run with: `cargo test -p lumos --release --features bench bench_moffat -- --ignored --nocapture`
use crate::internals::prelude::*;
use crate::star_detection::centroid::stamp::StampGrid;

use quickbench::quick_bench;
use std::hint::black_box;

use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::star_detection::centroid::moffat_fit::MoffatFit;

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
    let beta = 2.5;

    b.bench(|| {
        black_box(MoffatFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(8.0)),
            black_box(&StampGrid::new(8)),
            black_box(0.1),
            None,
            black_box(beta),
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
    let beta = 2.5;

    b.bench(|| {
        black_box(MoffatFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(12.0)),
            black_box(&StampGrid::new(12)),
            black_box(0.1),
            None,
            black_box(beta),
        ))
    });
}

/// The fit at each power strategy on a 17×17 stamp: β = 2.5 takes `u^n·√u`, 3 takes `u^n`, and
/// 2.3 the general power.
#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_moffat_fit_by_power(b: quickbench::Bencher) {
    for beta in [2.5f32, 3.0, 2.3] {
        let pixels = SyntheticStar::new(
            Vec2::new(8.3, 8.7),
            1.0,
            StarProfile::Moffat { alpha: 2.5, beta },
        )
        .stamp(Size2us::new(17, 17), 0.1);
        b.bench_labeled(&format!("beta {beta}"), || {
            black_box(MoffatFit::new(
                black_box(&pixels),
                black_box(DVec2::splat(8.0)),
                black_box(&StampGrid::new(8)),
                black_box(0.1),
                None,
                black_box(beta),
            ))
        });
    }
}
