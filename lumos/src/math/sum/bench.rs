//! Benchmarks comparing scalar against the vector backends.
//!
//! Every sweep benches the backend by name as well as the dispatcher, on both architectures, so a
//! kernel can be measured against scalar *below* its own gate — which is the measurement that sets
//! the gate. A dispatch-only label cannot do that: under the gate it is the scalar path.

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::math::sum::{scalar, sum_f32, weighted_mean_f32};

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

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_sum_f32(b: ::quickbench::Bencher) {
    let data = make_test_data();

    b.bench_labeled("scalar", || black_box(scalar::sum_f32(black_box(&data))));

    #[cfg(target_arch = "aarch64")]
    b.bench_labeled("neon", || unsafe {
        black_box(crate::math::sum::neon::sum_f32(black_box(&data)))
    });

    #[cfg(target_arch = "x86_64")]
    if imaginarium::cpu_features::has_avx2() {
        b.bench_labeled("avx2", || unsafe {
            black_box(crate::math::sum::avx2::sum_f32(black_box(&data)))
        });
    }

    b.bench_labeled("dispatch", || black_box(sum_f32(black_box(&data))));
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_weighted_mean_f32(b: ::quickbench::Bencher) {
    let data = make_test_data();
    let weights = make_weights();

    b.bench_labeled("scalar", || {
        black_box(scalar::weighted_sums(black_box(&data), black_box(&weights)))
    });

    #[cfg(target_arch = "aarch64")]
    b.bench_labeled("neon", || unsafe {
        black_box(crate::math::sum::neon::weighted_sums(
            black_box(&data),
            black_box(&weights),
        ))
    });

    #[cfg(target_arch = "x86_64")]
    if imaginarium::cpu_features::has_avx2() {
        b.bench_labeled("avx2", || unsafe {
            black_box(crate::math::sum::avx2::weighted_sums(
                black_box(&data),
                black_box(&weights),
            ))
        });
    }

    b.bench_labeled("dispatch", || {
        black_box(weighted_mean_f32(black_box(&data), black_box(&weights)))
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_sum_f32_crossover(b: ::quickbench::Bencher) {
    for len in CROSSOVER_SIZES {
        let data: Vec<f32> = (0..len).map(|x| x as f32 * 0.1).collect();
        let calls = calls_per_sample(len);

        b.bench_labeled(&format!("scalar_{len}"), || {
            for _ in 0..calls {
                black_box(scalar::sum_f32(black_box(&data)));
            }
        });

        #[cfg(target_arch = "aarch64")]
        b.bench_labeled(&format!("neon_{len}"), || {
            for _ in 0..calls {
                black_box(unsafe { crate::math::sum::neon::sum_f32(black_box(&data)) });
            }
        });

        #[cfg(target_arch = "x86_64")]
        if imaginarium::cpu_features::has_avx2() {
            b.bench_labeled(&format!("avx2_{len}"), || {
                for _ in 0..calls {
                    black_box(unsafe { crate::math::sum::avx2::sum_f32(black_box(&data)) });
                }
            });
        }
    }
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_weighted_sums_crossover(b: ::quickbench::Bencher) {
    for len in CROSSOVER_SIZES {
        let data: Vec<f32> = (0..len).map(|x| x as f32 * 0.1).collect();
        let weights: Vec<f32> = (0..len).map(|x| 1.0 + x as f32 * 0.01).collect();
        let calls = calls_per_sample(len);

        b.bench_labeled(&format!("scalar_{len}"), || {
            for _ in 0..calls {
                black_box(scalar::weighted_sums(black_box(&data), black_box(&weights)));
            }
        });

        #[cfg(target_arch = "aarch64")]
        b.bench_labeled(&format!("neon_{len}"), || {
            for _ in 0..calls {
                black_box(unsafe {
                    crate::math::sum::neon::weighted_sums(black_box(&data), black_box(&weights))
                });
            }
        });

        #[cfg(target_arch = "x86_64")]
        if imaginarium::cpu_features::has_avx2() {
            b.bench_labeled(&format!("avx2_{len}"), || {
                for _ in 0..calls {
                    black_box(unsafe {
                        crate::math::sum::avx2::weighted_sums(black_box(&data), black_box(&weights))
                    });
                }
            });
        }
    }
}
