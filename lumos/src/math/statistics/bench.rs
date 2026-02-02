//! Benchmarks for statistical functions.

use ::bench::quick_bench;
use std::hint::black_box;

use super::*;

const BENCH_SIZE: usize = 1024;
const TILE_SIZE: usize = 4096; // 64x64 tile - realistic background estimation size

fn make_test_data() -> Vec<f32> {
    (0..BENCH_SIZE).map(|x| 100.0 + (x % 20) as f32).collect()
}

fn make_test_data_with_outliers() -> Vec<f32> {
    let mut data: Vec<f32> = vec![100.0; BENCH_SIZE - 10];
    data.extend([1000.0, 2000.0, 3000.0, 4000.0, 5000.0]);
    data.extend([0.0, 1.0, 2.0, 3.0, 4.0]);
    data
}

fn make_tile_data_with_outliers() -> Vec<f32> {
    // Realistic tile: mostly uniform with ~2% outliers (stars)
    let mut data: Vec<f32> = vec![100.0; TILE_SIZE - 80];
    // Add some realistic star pixels (high outliers)
    data.extend(std::iter::repeat_n(500.0, 40));
    data.extend(std::iter::repeat_n(1000.0, 20));
    data.extend(std::iter::repeat_n(2000.0, 10));
    data.extend(std::iter::repeat_n(5000.0, 5));
    // Add some noise (low outliers)
    data.extend(std::iter::repeat_n(50.0, 5));
    data
}

#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_median_f32(b: ::bench::Bencher) {
    let data = make_test_data();

    b.bench(|| {
        let mut d = data.clone();
        black_box(median_f32_mut(black_box(&mut d)))
    });
}

#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_sigma_clipped_median_mad(b: ::bench::Bencher) {
    let data = make_test_data_with_outliers();
    let mut deviations = Vec::with_capacity(BENCH_SIZE);

    b.bench(|| {
        let mut d = data.clone();
        black_box(sigma_clipped_median_mad(
            black_box(&mut d),
            black_box(&mut deviations),
            3.0,
            3,
        ))
    });
}

/// Benchmark sigma clipping on realistic 64x64 tile (4096 pixels, 5 iterations)
#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_sigma_clipped_tile_4096(b: ::bench::Bencher) {
    let data = make_tile_data_with_outliers();
    let mut deviations = Vec::with_capacity(TILE_SIZE);

    b.bench(|| {
        let mut d = data.clone();
        black_box(sigma_clipped_median_mad(
            black_box(&mut d),
            black_box(&mut deviations),
            3.0,
            5, // 5 iterations like default config
        ))
    });
}
