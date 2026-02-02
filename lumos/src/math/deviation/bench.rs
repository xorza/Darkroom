//! Benchmarks comparing scalar vs SIMD deviation operations.

use ::bench::quick_bench;
use std::hint::black_box;

use super::abs_deviation_inplace;
use super::scalar;

const BENCH_SIZE: usize = 10000;

fn make_test_data() -> Vec<f32> {
    (0..BENCH_SIZE).map(|x| x as f32 * 0.1).collect()
}

#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_abs_deviation_inplace(b: ::bench::Bencher) {
    let original = make_test_data();
    let median = 500.0;

    b.bench_labeled("scalar", || {
        let mut data = original.clone();
        scalar::abs_deviation_inplace(black_box(&mut data), median);
        black_box(data)
    });

    b.bench_labeled("simd", || {
        let mut data = original.clone();
        abs_deviation_inplace(black_box(&mut data), median);
        black_box(data)
    });
}
