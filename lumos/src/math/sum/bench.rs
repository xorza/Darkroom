//! Benchmarks comparing scalar vs SIMD sum operations.

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::math::sum::scalar;
use crate::math::sum::sum_f32;
use crate::math::sum::weighted_mean_f32;

const BENCH_SIZE: usize = 10_000;
const CROSSOVER_SIZES: [usize; 15] = [
    1, 2, 3, 4, 8, 16, 32, 64, 128, 256, 512, 1_024, 2_048, 4_096, 10_000,
];

fn make_test_data() -> Vec<f32> {
    (0..BENCH_SIZE).map(|x| x as f32 * 0.1).collect()
}

fn make_weights() -> Vec<f32> {
    (0..BENCH_SIZE).map(|x| 1.0 + (x as f32) * 0.001).collect()
}

fn calls_per_sample(len: usize) -> usize {
    (8_192 / len).clamp(1, 2_048)
}

#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_sum_f32(b: ::quickbench::Bencher) {
    let data = make_test_data();

    b.bench_labeled("scalar", || black_box(scalar::sum_f32(black_box(&data))));
    b.bench_labeled("simd", || black_box(sum_f32(black_box(&data))));
}

#[quick_bench(warmup_iters = 3, iters = 100)]
fn bench_weighted_mean_f32(b: ::quickbench::Bencher) {
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
        use crate::math::sum::sse;
        if common::cpu_features::has_sse4_1() {
            b.bench_labeled("sse", || unsafe {
                black_box(sse::weighted_mean_f32(
                    black_box(&data),
                    black_box(&weights),
                ))
            });
        }

        use crate::math::sum::avx2;
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

#[quick_bench(warmup_iters = 10, iters = 200)]
fn bench_sum_f32_crossover(b: ::quickbench::Bencher) {
    for len in CROSSOVER_SIZES {
        let data: Vec<f32> = (0..len).map(|x| x as f32 * 0.1).collect();
        let calls = calls_per_sample(len);

        b.bench_labeled(&format!("scalar_{len}"), || {
            for _ in 0..calls {
                black_box(scalar::sum_f32(black_box(&data)));
            }
        });

        #[cfg(target_arch = "x86_64")]
        {
            use crate::math::sum::avx2;

            if common::cpu_features::has_avx2() {
                b.bench_labeled(&format!("avx2_{len}"), || {
                    for _ in 0..calls {
                        black_box(unsafe { avx2::sum_f32(black_box(&data)) });
                    }
                });
            }
        }

        b.bench_labeled(&format!("dispatch_{len}"), || {
            for _ in 0..calls {
                black_box(sum_f32(black_box(&data)));
            }
        });
    }
}

#[quick_bench(warmup_iters = 10, iters = 200)]
fn bench_weighted_mean_f32_crossover(b: ::quickbench::Bencher) {
    for len in CROSSOVER_SIZES {
        let data: Vec<f32> = (0..len).map(|x| x as f32 * 0.1).collect();
        let weights: Vec<f32> = (0..len).map(|x| 1.0 + x as f32 * 0.01).collect();
        let calls = calls_per_sample(len);

        b.bench_labeled(&format!("scalar_{len}"), || {
            for _ in 0..calls {
                black_box(scalar::weighted_mean_f32(
                    black_box(&data),
                    black_box(&weights),
                ));
            }
        });

        #[cfg(target_arch = "x86_64")]
        {
            use crate::math::sum::{avx2, sse};

            if common::cpu_features::has_sse4_1() {
                b.bench_labeled(&format!("sse_{len}"), || {
                    for _ in 0..calls {
                        black_box(unsafe {
                            sse::weighted_mean_f32(black_box(&data), black_box(&weights))
                        });
                    }
                });
            }
            if common::cpu_features::has_avx2() {
                b.bench_labeled(&format!("avx2_{len}"), || {
                    for _ in 0..calls {
                        black_box(unsafe {
                            avx2::weighted_mean_f32(black_box(&data), black_box(&weights))
                        });
                    }
                });
            }
        }

        b.bench_labeled(&format!("dispatch_{len}"), || {
            for _ in 0..calls {
                black_box(weighted_mean_f32(black_box(&data), black_box(&weights)));
            }
        });
    }
}
