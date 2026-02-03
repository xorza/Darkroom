//! Benchmarks comparing SIMD implementations of centroid refinement.
//!
//! Run with: `cargo test -p lumos --release bench_simd_centroid -- --ignored --nocapture`

use ::bench::quick_bench;
use common::cpu_features;
use glam::Vec2;
use std::hint::black_box;

use super::scalar::refine_centroid_scalar;
use crate::common::Buffer2;
use crate::star_detection::background::BackgroundConfig;

#[cfg(target_arch = "x86_64")]
use super::avx2::refine_centroid_avx2;
#[cfg(target_arch = "x86_64")]
use super::sse::refine_centroid_sse;

/// Create a synthetic Gaussian star for benchmarking.
fn make_gaussian_star(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    sigma: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r2 = dx * dx + dy * dy;
            let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }
    Buffer2::new(width, height, pixels)
}

// =============================================================================
// SIMD Implementation Comparison Benchmarks
// =============================================================================

/// Compare all SIMD implementations: scalar vs SSE vs AVX2 (vs NEON on ARM).
#[quick_bench(warmup_iters = 10, iters = 10000)]
fn bench_simd_centroid_comparison(b: ::bench::Bencher) {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, 32.3, 32.7, 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let stamp_radius = 7; // typical for FWHM ~4
    let expected_fwhm = 4.0;

    // Scalar baseline
    b.bench_labeled("scalar", || {
        black_box(refine_centroid_scalar(
            black_box(&*pixels),
            black_box(width),
            black_box(height),
            black_box(&bg),
            black_box(Vec2::splat(32.0)),
            black_box(stamp_radius),
            black_box(expected_fwhm),
        ))
    });

    // SSE implementation (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4.1", || {
            // SAFETY: SSE4.1 is available (checked above)
            unsafe {
                black_box(refine_centroid_sse(
                    black_box(&*pixels),
                    black_box(width),
                    black_box(height),
                    black_box(&bg),
                    black_box(Vec2::splat(32.0)),
                    black_box(stamp_radius),
                    black_box(expected_fwhm),
                ))
            }
        });
    }

    // AVX2 implementation (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2+fma", || {
            // SAFETY: AVX2 and FMA are available (checked above)
            unsafe {
                black_box(refine_centroid_avx2(
                    black_box(&*pixels),
                    black_box(width),
                    black_box(height),
                    black_box(&bg),
                    black_box(Vec2::splat(32.0)),
                    black_box(stamp_radius),
                    black_box(expected_fwhm),
                ))
            }
        });
    }

    // Note: NEON benchmark only runs on aarch64
    #[cfg(target_arch = "aarch64")]
    {
        use super::neon::refine_centroid_neon;
        b.bench_labeled("neon", || {
            // SAFETY: NEON is always available on aarch64
            unsafe {
                black_box(refine_centroid_neon(
                    black_box(&*pixels),
                    black_box(width),
                    black_box(height),
                    black_box(&bg),
                    black_box(Vec2::splat(32.0)),
                    black_box(stamp_radius),
                    black_box(expected_fwhm),
                ))
            }
        });
    }
}

/// Batch benchmark: 1000 centroids to amplify differences.
#[quick_bench(warmup_iters = 5, iters = 100)]
fn bench_simd_centroid_batch_1000(b: ::bench::Bencher) {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, 32.3, 32.7, 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let stamp_radius = 7;
    let expected_fwhm = 4.0;

    // Scalar baseline
    b.bench_labeled("scalar", || {
        for _ in 0..1000 {
            black_box(refine_centroid_scalar(
                black_box(&*pixels),
                black_box(width),
                black_box(height),
                black_box(&bg),
                black_box(Vec2::splat(32.0)),
                black_box(stamp_radius),
                black_box(expected_fwhm),
            ));
        }
    });

    // SSE implementation (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4.1", || {
            for _ in 0..1000 {
                // SAFETY: SSE4.1 is available (checked above)
                unsafe {
                    black_box(refine_centroid_sse(
                        black_box(&*pixels),
                        black_box(width),
                        black_box(height),
                        black_box(&bg),
                        black_box(Vec2::splat(32.0)),
                        black_box(stamp_radius),
                        black_box(expected_fwhm),
                    ));
                }
            }
        });
    }

    // AVX2 implementation (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2+fma", || {
            for _ in 0..1000 {
                // SAFETY: AVX2 and FMA are available (checked above)
                unsafe {
                    black_box(refine_centroid_avx2(
                        black_box(&*pixels),
                        black_box(width),
                        black_box(height),
                        black_box(&bg),
                        black_box(Vec2::splat(32.0)),
                        black_box(stamp_radius),
                        black_box(expected_fwhm),
                    ));
                }
            }
        });
    }

    // Note: NEON benchmark only runs on aarch64
    #[cfg(target_arch = "aarch64")]
    {
        use super::neon::refine_centroid_neon;
        b.bench_labeled("neon", || {
            for _ in 0..1000 {
                // SAFETY: NEON is always available on aarch64
                unsafe {
                    black_box(refine_centroid_neon(
                        black_box(&*pixels),
                        black_box(width),
                        black_box(height),
                        black_box(&bg),
                        black_box(Vec2::splat(32.0)),
                        black_box(stamp_radius),
                        black_box(expected_fwhm),
                    ));
                }
            }
        });
    }
}

/// Different stamp sizes to test SIMD efficiency at various scales.
#[quick_bench(warmup_iters = 5, iters = 5000)]
fn bench_simd_centroid_stamp_sizes(b: ::bench::Bencher) {
    let width = 128;
    let height = 128;
    let pixels = make_gaussian_star(width, height, 64.3, 64.7, 4.0, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());

    // Small stamp (9x9) - remainder handling matters more
    let stamp_radius_small = 4;
    let expected_fwhm_small = 3.0;

    // Medium stamp (15x15) - good balance
    let stamp_radius_med = 7;
    let expected_fwhm_med = 5.0;

    // Large stamp (21x21) - SIMD shines
    let stamp_radius_large = 10;
    let expected_fwhm_large = 7.0;

    // Scalar at different stamp sizes
    b.bench_labeled("scalar_9x9", || {
        black_box(refine_centroid_scalar(
            black_box(&*pixels),
            black_box(width),
            black_box(height),
            black_box(&bg),
            black_box(Vec2::splat(64.0)),
            black_box(stamp_radius_small),
            black_box(expected_fwhm_small),
        ))
    });

    b.bench_labeled("scalar_15x15", || {
        black_box(refine_centroid_scalar(
            black_box(&*pixels),
            black_box(width),
            black_box(height),
            black_box(&bg),
            black_box(Vec2::splat(64.0)),
            black_box(stamp_radius_med),
            black_box(expected_fwhm_med),
        ))
    });

    b.bench_labeled("scalar_21x21", || {
        black_box(refine_centroid_scalar(
            black_box(&*pixels),
            black_box(width),
            black_box(height),
            black_box(&bg),
            black_box(Vec2::splat(64.0)),
            black_box(stamp_radius_large),
            black_box(expected_fwhm_large),
        ))
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2_9x9", || unsafe {
            black_box(refine_centroid_avx2(
                black_box(&*pixels),
                black_box(width),
                black_box(height),
                black_box(&bg),
                black_box(Vec2::splat(64.0)),
                black_box(stamp_radius_small),
                black_box(expected_fwhm_small),
            ))
        });

        b.bench_labeled("avx2_15x15", || unsafe {
            black_box(refine_centroid_avx2(
                black_box(&*pixels),
                black_box(width),
                black_box(height),
                black_box(&bg),
                black_box(Vec2::splat(64.0)),
                black_box(stamp_radius_med),
                black_box(expected_fwhm_med),
            ))
        });

        b.bench_labeled("avx2_21x21", || unsafe {
            black_box(refine_centroid_avx2(
                black_box(&*pixels),
                black_box(width),
                black_box(height),
                black_box(&bg),
                black_box(Vec2::splat(64.0)),
                black_box(stamp_radius_large),
                black_box(expected_fwhm_large),
            ))
        });
    }
}
