//! Benchmarks comparing scalar vs SIMD sum operations.

use ::bench::quick_bench;
use std::hint::black_box;

use super::scalar;
use super::sum_f32;

const BENCH_SIZE: usize = 10000;

fn make_test_data() -> Vec<f32> {
    (0..BENCH_SIZE).map(|x| x as f32 * 0.1).collect()
}

#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_sum_f32(b: ::bench::Bencher) {
    let data = make_test_data();

    b.bench_labeled("scalar", || black_box(scalar::sum_f32(black_box(&data))));
    b.bench_labeled("simd", || black_box(sum_f32(black_box(&data))));
}
