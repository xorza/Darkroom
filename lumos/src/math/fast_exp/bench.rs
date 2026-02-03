//! Benchmarks comparing fast_exp implementations: scalar libm vs SSE vs AVX2.
//!
//! Run with: `cargo test -p lumos --release bench_fast_exp -- --ignored --nocapture`
//!
//! Note: SSE/AVX2 benchmarks use the register-based `_m128`/`_m256` variants
//! directly (loading from contiguous memory via SIMD loads) to reflect real-world
//! usage in gaussian_fit, where data stays in SIMD registers throughout.

use ::bench::quick_bench;
use std::hint::black_box;

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

/// Horizontal sum of an __m128 register (4 floats).
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "sse4.1")]
unsafe fn hsum_m128(v: std::arch::x86_64::__m128) -> f32 {
    use std::arch::x86_64::*;
    let hi = _mm_movehdup_ps(v); // [1,1,3,3]
    let sum1 = _mm_add_ps(v, hi); // [0+1,_,2+3,_]
    let hi2 = _mm_movehl_ps(sum1, sum1); // [2+3,_,_,_]
    let sum2 = _mm_add_ss(sum1, hi2);
    _mm_cvtss_f32(sum2)
}

/// Horizontal sum of an __m256 register (8 floats).
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn hsum_m256(v: std::arch::x86_64::__m256) -> f32 {
    use std::arch::x86_64::*;
    let lo = _mm256_castps256_ps128(v);
    let hi = _mm256_extractf128_ps(v, 1);
    let sum = _mm_add_ps(lo, hi);
    unsafe { hsum_m128(sum) }
}

/// Generate test exponent values typical for Gaussian fitting.
/// Exponents range from -20 (far from center, negligible weight) to 0 (at center).
fn make_gaussian_exponents(n: usize) -> Vec<f32> {
    (0..n).map(|i| -20.0 * (i as f32) / (n as f32)).collect()
}

/// Generate test exponent values covering a wider range [-80, 20].
fn make_wide_range_exponents(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| -80.0 + 100.0 * (i as f32) / (n as f32))
        .collect()
}

// ============================================================================
// Gaussian range benchmarks (typical L-M fitting inputs)
// ============================================================================

#[quick_bench(warmup_iters = 50, iters = 100000)]
fn bench_fast_exp_gaussian_range(b: ::bench::Bencher) {
    let values = make_gaussian_exponents(1000);

    b.bench_labeled("libm_scalar", || {
        let mut sum = 0.0f32;
        for &x in black_box(&values) {
            sum += x.exp();
        }
        black_box(sum)
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            use std::arch::x86_64::*;
            let mut sum = 0.0f32;
            let ptr = black_box(&values).as_ptr();
            for i in (0..values.len()).step_by(4) {
                let vx = unsafe { _mm_loadu_ps(ptr.add(i)) };
                let result = unsafe { super::sse::fast_exp_4_sse_m128(vx) };
                sum += unsafe { hsum_m128(result) };
            }
            black_box(sum)
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            use std::arch::x86_64::*;
            let mut sum = 0.0f32;
            let ptr = black_box(&values).as_ptr();
            for i in (0..values.len()).step_by(8) {
                let vx = unsafe { _mm256_loadu_ps(ptr.add(i)) };
                let result = unsafe { super::avx2::fast_exp_8_avx2_m256(vx) };
                sum += unsafe { hsum_m256(result) };
            }
            black_box(sum)
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let mut sum = 0.0f32;
            for chunk in black_box(&values).chunks_exact(4) {
                let batch: [f32; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
                let result = unsafe { super::fast_exp_4_neon(&batch) };
                sum += result[0] + result[1] + result[2] + result[3];
            }
            black_box(sum)
        });
    }
}

// ============================================================================
// Wide range benchmarks (stress test across full f32 exp domain)
// ============================================================================

#[quick_bench(warmup_iters = 50, iters = 100000)]
fn bench_fast_exp_wide_range(b: ::bench::Bencher) {
    let values = make_wide_range_exponents(1000);

    b.bench_labeled("libm_scalar", || {
        let mut sum = 0.0f32;
        for &x in black_box(&values) {
            sum += x.exp();
        }
        black_box(sum)
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            use std::arch::x86_64::*;
            let mut sum = 0.0f32;
            let ptr = black_box(&values).as_ptr();
            for i in (0..values.len()).step_by(4) {
                let vx = unsafe { _mm_loadu_ps(ptr.add(i)) };
                let result = unsafe { super::sse::fast_exp_4_sse_m128(vx) };
                sum += unsafe { hsum_m128(result) };
            }
            black_box(sum)
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            use std::arch::x86_64::*;
            let mut sum = 0.0f32;
            let ptr = black_box(&values).as_ptr();
            for i in (0..values.len()).step_by(8) {
                let vx = unsafe { _mm256_loadu_ps(ptr.add(i)) };
                let result = unsafe { super::avx2::fast_exp_8_avx2_m256(vx) };
                sum += unsafe { hsum_m256(result) };
            }
            black_box(sum)
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let mut sum = 0.0f32;
            for chunk in black_box(&values).chunks_exact(4) {
                let batch: [f32; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
                let result = unsafe { super::fast_exp_4_neon(&batch) };
                sum += result[0] + result[1] + result[2] + result[3];
            }
            black_box(sum)
        });
    }
}

// ============================================================================
// Batch processing benchmark (simulates per-star stamp processing)
// ============================================================================

#[quick_bench(warmup_iters = 10, iters = 1000)]
fn bench_fast_exp_batch_stamps(b: ::bench::Bencher) {
    // Simulate 1000 stars, each with a 17x17=289 pixel stamp
    let n_stars = 1000;
    let stamp_size = 289;
    let stamps: Vec<Vec<f32>> = (0..n_stars)
        .map(|s| {
            (0..stamp_size)
                .map(|i| {
                    let dx = (i % 17) as f32 - 8.0;
                    let dy = (i / 17) as f32 - 8.0;
                    let sigma = 2.5 + (s % 5) as f32 * 0.5;
                    -0.5 * (dx * dx + dy * dy) / (sigma * sigma)
                })
                .collect()
        })
        .collect();

    b.bench_labeled("libm_scalar", || {
        let mut sum = 0.0f32;
        for stamp in black_box(&stamps) {
            for &x in stamp {
                sum += x.exp();
            }
        }
        black_box(sum)
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            use std::arch::x86_64::*;
            let mut sum = 0.0f32;
            for stamp in black_box(&stamps) {
                let ptr = stamp.as_ptr();
                for i in (0..stamp.len()).step_by(4) {
                    let vx = unsafe { _mm_loadu_ps(ptr.add(i)) };
                    let result = unsafe { super::sse::fast_exp_4_sse_m128(vx) };
                    sum += unsafe { hsum_m128(result) };
                }
            }
            black_box(sum)
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            use std::arch::x86_64::*;
            let mut sum = 0.0f32;
            for stamp in black_box(&stamps) {
                let ptr = stamp.as_ptr();
                for i in (0..stamp.len()).step_by(8) {
                    let vx = unsafe { _mm256_loadu_ps(ptr.add(i)) };
                    let result = unsafe { super::avx2::fast_exp_8_avx2_m256(vx) };
                    sum += unsafe { hsum_m256(result) };
                }
            }
            black_box(sum)
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let mut sum = 0.0f32;
            for stamp in black_box(&stamps) {
                for chunk in stamp.chunks_exact(4) {
                    let batch: [f32; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
                    let result = unsafe { super::fast_exp_4_neon(&batch) };
                    sum += result[0] + result[1] + result[2] + result[3];
                }
            }
            black_box(sum)
        });
    }
}
