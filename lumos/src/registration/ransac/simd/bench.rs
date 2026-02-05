//! Benchmarks comparing SIMD implementations of RANSAC inlier counting.
//!
//! Run with: `cargo test -p lumos --release bench_simd_inlier -- --ignored --nocapture`

use ::bench::quick_bench;
use glam::DVec2;
use std::hint::black_box;

use super::count_inliers_scalar;
use crate::registration::transform::Transform;

#[cfg(target_arch = "x86_64")]
use super::sse::{count_inliers_avx2, count_inliers_sse2};
#[cfg(target_arch = "x86_64")]
use common::cpu_features;

#[cfg(target_arch = "aarch64")]
use super::neon::count_inliers_neon;

/// Create test data: reference points, target points with some outliers.
fn make_test_data(num_points: usize, outlier_ratio: f64) -> (Vec<DVec2>, Vec<DVec2>, Transform) {
    let transform = Transform::similarity(DVec2::new(100.0, 50.0), 0.15, 1.05);

    let ref_points: Vec<DVec2> = (0..num_points)
        .map(|i| {
            let angle = i as f64 * 0.1;
            let radius = 100.0 + (i as f64 * 0.5);
            DVec2::new(radius * angle.cos(), radius * angle.sin())
        })
        .collect();

    let target_points: Vec<DVec2> = ref_points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let t = transform.apply(*p);
            // Add outliers at regular intervals
            if (i as f64) < (num_points as f64 * outlier_ratio) && i % 3 == 0 {
                t + DVec2::new(500.0, 500.0) // outlier
            } else {
                t + DVec2::new(0.05, -0.05) // small noise
            }
        })
        .collect();

    (ref_points, target_points, transform)
}

/// Compare all SIMD implementations: scalar vs SSE2 vs AVX2 (vs NEON on ARM).
#[quick_bench(warmup_iters = 10, iters = 10000)]
fn bench_simd_inlier_comparison(b: ::bench::Bencher) {
    let (ref_points, target_points, transform) = make_test_data(500, 0.2);
    let threshold_sq = 4.0; // threshold = 2.0
    let mut inliers = Vec::with_capacity(500);

    // Scalar baseline
    b.bench_labeled("scalar", || {
        black_box(count_inliers_scalar(
            black_box(&ref_points),
            black_box(&target_points),
            black_box(&transform),
            black_box(threshold_sq),
            black_box(&mut inliers),
        ))
    });

    // SSE2 implementation (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse2() {
        b.bench_labeled("sse2", || {
            // SAFETY: SSE2 is available (checked above)
            unsafe {
                black_box(count_inliers_sse2(
                    black_box(&ref_points),
                    black_box(&target_points),
                    black_box(&transform),
                    black_box(threshold_sq),
                    black_box(&mut inliers),
                ))
            }
        });
    }

    // AVX2 implementation (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2() {
        b.bench_labeled("avx2", || {
            // SAFETY: AVX2 is available (checked above)
            unsafe {
                black_box(count_inliers_avx2(
                    black_box(&ref_points),
                    black_box(&target_points),
                    black_box(&transform),
                    black_box(threshold_sq),
                    black_box(&mut inliers),
                ))
            }
        });
    }

    // NEON implementation (aarch64 only)
    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            // SAFETY: NEON is always available on aarch64
            unsafe {
                black_box(count_inliers_neon(
                    black_box(&ref_points),
                    black_box(&target_points),
                    black_box(&transform),
                    black_box(threshold_sq),
                    black_box(&mut inliers),
                ))
            }
        });
    }
}
