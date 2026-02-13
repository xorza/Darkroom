//! Benchmarks comparing scalar vs SIMD sum operations.

use ::bench::quick_bench;
use std::hint::black_box;

use super::scalar;
use super::sum_f32;
use super::weighted_mean_f32;

const BENCH_SIZE: usize = 10000;

fn make_test_data() -> Vec<f32> {
    (0..BENCH_SIZE).map(|x| x as f32 * 0.1).collect()
}

fn make_weights() -> Vec<f32> {
    (0..BENCH_SIZE).map(|x| 1.0 + (x as f32) * 0.001).collect()
}

#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_sum_f32(b: ::bench::Bencher) {
    let data = make_test_data();

    b.bench_labeled("scalar", || black_box(scalar::sum_f32(black_box(&data))));
    b.bench_labeled("simd", || black_box(sum_f32(black_box(&data))));
}

#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_weighted_mean_f32(b: ::bench::Bencher) {
    let data = make_test_data();
    let weights = make_weights();

    b.bench_labeled("scalar", || {
        black_box(scalar::weighted_mean_f32(
            black_box(&data),
            black_box(&weights),
        ))
    });

    #[cfg(target_arch = "x86_64")]
    {
        use super::sse;
        if common::cpu_features::has_sse4_1() {
            b.bench_labeled("sse", || unsafe {
                black_box(sse::weighted_mean_f32(
                    black_box(&data),
                    black_box(&weights),
                ))
            });
        }

        use super::avx2;
        if common::cpu_features::has_avx2() {
            b.bench_labeled("avx2", || unsafe {
                black_box(avx2::weighted_mean_f32(
                    black_box(&data),
                    black_box(&weights),
                ))
            });
        }
    }

    b.bench_labeled("dispatch", || {
        black_box(weighted_mean_f32(black_box(&data), black_box(&weights)))
    });
}
