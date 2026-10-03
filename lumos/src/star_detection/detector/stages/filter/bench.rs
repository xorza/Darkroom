//! Benchmarks for duplicate-star removal.

use ::quickbench::quick_bench;
use std::hint::black_box;

use glam::DVec2;

use crate::star_detection::detector::stages::filter::remove_duplicate_stars;
use crate::star_detection::roundness::Roundness;
use crate::star_detection::star::Star;

/// `count` stars scattered over a `width` × `height` frame, every property randomized across the
/// range a real detection would produce.
fn random_stars(count: usize, width: f64, height: f64) -> Vec<Star> {
    use rand::prelude::*;

    let mut rng = StdRng::seed_from_u64(42);
    (0..count)
        .map(|_| {
            Star::at(DVec2::new(
                rng.random_range(0.0..width),
                rng.random_range(0.0..height),
            ))
            .with_flux(rng.random_range(100.0..10000.0))
            .with_fwhm(rng.random_range(2.0..6.0))
            .with_eccentricity(rng.random_range(0.0..0.3))
            .with_snr(rng.random_range(10.0..100.0))
            .with_peak(rng.random_range(0.1..0.9))
            .with_sharpness(rng.random_range(0.2..0.5))
            .with_roundness(Roundness {
                ground: rng.random_range(-0.1..0.1),
                sround: rng.random_range(-0.1..0.1),
            })
        })
        .collect()
}

fn bench_deduplication(b: ::quickbench::Bencher, base_stars: &[Star]) {
    b.bench(|| {
        let mut stars = base_stars.to_vec();
        // Sort by flux — the algorithm's documented precondition.
        stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());
        black_box(remove_duplicate_stars(&mut stars, 5.0))
    });
}

/// Benchmark remove_duplicate_stars with varying star counts.
/// Simulates dense star field scenario similar to rho-opiuchi detection.
#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_remove_duplicate_stars_5000(b: ::quickbench::Bencher) {
    bench_deduplication(b, &random_stars(5000, 4096.0, 4096.0));
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_remove_duplicate_stars_10000(b: ::quickbench::Bencher) {
    bench_deduplication(b, &random_stars(10000, 8000.0, 6000.0));
}
