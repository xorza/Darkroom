//! Benchmarks comparing fast_pow_neg_beta vs powf.

use ::bench::quick_bench;
use std::hint::black_box;

use super::fast_pow_neg_beta;

/// Generate test u values typical for Moffat profile fitting.
/// u = 1 + r²/α² ranges from 1.0 (center) to ~50 (stamp edge).
fn make_u_values(n: usize) -> Vec<f32> {
    (0..n).map(|i| 1.0 + (i as f32) * 0.05).collect()
}

#[quick_bench(warmup_iters = 50, iters = 10000)]
fn bench_pow_neg_beta_half_integer(b: ::bench::Bencher) {
    let u_values = make_u_values(1000);
    let neg_beta = -2.5f32;

    b.bench_labeled("fast", || {
        let mut sum = 0.0f32;
        for &u in black_box(&u_values) {
            sum += fast_pow_neg_beta(u, neg_beta);
        }
        black_box(sum)
    });

    b.bench_labeled("powf", || {
        let mut sum = 0.0f32;
        for &u in black_box(&u_values) {
            sum += u.powf(neg_beta);
        }
        black_box(sum)
    });

    b.bench_labeled("exp_ln", || {
        let mut sum = 0.0f32;
        for &u in black_box(&u_values) {
            sum += (neg_beta * u.ln()).exp();
        }
        black_box(sum)
    });
}

#[quick_bench(warmup_iters = 50, iters = 10000)]
fn bench_pow_neg_beta_arbitrary(b: ::bench::Bencher) {
    let u_values = make_u_values(1000);
    let neg_beta = -2.7f32; // Non-half-integer, falls back to powf

    b.bench_labeled("fast", || {
        let mut sum = 0.0f32;
        for &u in black_box(&u_values) {
            sum += fast_pow_neg_beta(u, neg_beta);
        }
        black_box(sum)
    });

    b.bench_labeled("powf", || {
        let mut sum = 0.0f32;
        for &u in black_box(&u_values) {
            sum += u.powf(neg_beta);
        }
        black_box(sum)
    });
}
