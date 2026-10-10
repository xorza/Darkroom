//! Benchmarks for statistical functions.

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::math::statistics::*;

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
    data.extend(iter::repeat_n(500.0, 40));
    data.extend(iter::repeat_n(1000.0, 20));
    data.extend(iter::repeat_n(2000.0, 10));
    data.extend(iter::repeat_n(5000.0, 5));
    // Add some noise (low outliers)
    data.extend(iter::repeat_n(50.0, 5));
    data
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_median_f32(b: ::quickbench::Bencher) {
    let data = make_test_data();

    b.bench(|| {
        let mut d = data.clone();
        black_box(median_mut(black_box(&mut d)))
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_sigma_clipped_median_mad(b: ::quickbench::Bencher) {
    let data = make_test_data_with_outliers();
    let mut deviations = Vec::with_capacity(BENCH_SIZE);

    b.bench(|| {
        let mut d = data.clone();
        black_box(ClippedStats::sigma_clipped(
            black_box(&mut d),
            black_box(&mut deviations),
            3.0,
            3,
        ))
    });
}

/// Benchmark sigma clipping on realistic 64x64 tile (4096 pixels, 5 iterations)
#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_sigma_clipped_tile_4096(b: ::quickbench::Bencher) {
    let data = make_tile_data_with_outliers();
    let mut deviations = Vec::with_capacity(TILE_SIZE);

    b.bench(|| {
        let mut d = data.clone();
        black_box(ClippedStats::sigma_clipped(
            black_box(&mut d),
            black_box(&mut deviations),
            3.0,
            5, // 5 iterations like default config
        ))
    });
}

/// A 24 MP plane of sky at 0.1 with ±0.01 of hashed noise: what normalization ranks per plane.
fn make_plane() -> Vec<f32> {
    (0..24_000_000u32)
        .map(|index| {
            let hash = index.wrapping_mul(0x9E37_79B9).rotate_left(13) ^ index;
            0.1 + (hash % 2001) as f32 * 1e-5 - 0.01
        })
        .collect()
}

/// A plane's median by copying it and selecting, as normalization did.
#[quick_bench(warmup_time_ms = 100, bench_time_ms = 2000)]
fn bench_plane_median_by_selection(b: ::quickbench::Bencher) {
    let plane = make_plane();
    let mut copy = Vec::with_capacity(plane.len());
    b.bench(|| {
        copy.clear();
        copy.extend_from_slice(black_box(&plane));
        black_box(median_mut(&mut copy))
    });
}

/// The same median by two radix passes over the plane, with no copy.
#[quick_bench(warmup_time_ms = 100, bench_time_ms = 2000)]
fn bench_plane_median_by_radix(b: ::quickbench::Bencher) {
    let plane = make_plane();
    let mut median = radix_median::RadixMedian::default();
    b.bench(|| {
        let mut high = median.high_pass();
        for &value in black_box(&plane) {
            high.add(value);
        }
        let mut low = high.finish();
        for &value in &plane {
            low.add(value);
        }
        black_box(low.median())
    });
}
