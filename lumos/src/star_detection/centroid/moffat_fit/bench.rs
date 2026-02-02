//! Benchmarks for Moffat SIMD vs Scalar comparison.
//!
//! Run with: `cargo test -p lumos --release bench_moffat_simd -- --ignored --nocapture`

use bench::quick_bench;
use std::hint::black_box;

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

use super::simd;

/// Generate test data for Moffat profile fitting.
fn generate_moffat_data(
    n_pixels: usize,
    cx: f32,
    cy: f32,
    amp: f32,
    alpha: f32,
    beta: f32,
    bg: f32,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let side = (n_pixels as f32).sqrt().ceil() as usize;
    let mut data_x = Vec::with_capacity(n_pixels);
    let mut data_y = Vec::with_capacity(n_pixels);
    let mut data_z = Vec::with_capacity(n_pixels);

    let alpha2 = alpha * alpha;
    for i in 0..n_pixels {
        let x = (i % side) as f32;
        let y = (i / side) as f32;
        let r2 = (x - cx).powi(2) + (y - cy).powi(2);
        let z = amp * (1.0 + r2 / alpha2).powf(-beta) + bg;
        data_x.push(x);
        data_y.push(y);
        data_z.push(z + 0.001 * (i as f32 % 7.0)); // Small noise
    }

    (data_x, data_y, data_z)
}

// ============================================================================
// Jacobian benchmarks - compare all implementations
// ============================================================================

#[quick_bench(warmup_iters = 100, iters = 1000)]
fn bench_moffat_jacobian_small(b: bench::Bencher) {
    // Small stamp: 17x17 = 289 pixels
    let (data_x, data_y, data_z) = generate_moffat_data(289, 8.5, 8.5, 1.0, 2.5, 2.5, 0.1);
    let params = [8.5f32, 8.5, 1.0, 2.5, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        simd::fill_jacobian_residuals_scalar(
            &data_x,
            &data_y,
            &data_z,
            &params,
            beta,
            &mut jacobian,
            &mut residuals,
        );
        black_box(jacobian.len() + residuals.len())
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::sse::fill_jacobian_residuals_sse_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::avx2::fill_jacobian_residuals_simd_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::neon::fill_jacobian_residuals_neon_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }
}

#[quick_bench(warmup_iters = 50, iters = 500)]
fn bench_moffat_jacobian_medium(b: bench::Bencher) {
    // Medium stamp: 25x25 = 625 pixels
    let (data_x, data_y, data_z) = generate_moffat_data(625, 12.5, 12.5, 1.0, 3.0, 2.5, 0.1);
    let params = [12.5f32, 12.5, 1.0, 3.0, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        simd::fill_jacobian_residuals_scalar(
            &data_x,
            &data_y,
            &data_z,
            &params,
            beta,
            &mut jacobian,
            &mut residuals,
        );
        black_box(jacobian.len() + residuals.len())
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::sse::fill_jacobian_residuals_sse_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::avx2::fill_jacobian_residuals_simd_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::neon::fill_jacobian_residuals_neon_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }
}

#[quick_bench(warmup_iters = 20, iters = 200)]
fn bench_moffat_jacobian_large(b: bench::Bencher) {
    // Large stamp: 33x33 = 1089 pixels
    let (data_x, data_y, data_z) = generate_moffat_data(1089, 16.5, 16.5, 1.0, 4.0, 2.5, 0.1);
    let params = [16.5f32, 16.5, 1.0, 4.0, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        simd::fill_jacobian_residuals_scalar(
            &data_x,
            &data_y,
            &data_z,
            &params,
            beta,
            &mut jacobian,
            &mut residuals,
        );
        black_box(jacobian.len() + residuals.len())
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::sse::fill_jacobian_residuals_sse_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::avx2::fill_jacobian_residuals_simd_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                simd::neon::fill_jacobian_residuals_neon_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
            black_box(jacobian.len() + residuals.len())
        });
    }
}

// ============================================================================
// Chi² benchmarks - compare all implementations
// ============================================================================

#[quick_bench(warmup_iters = 100, iters = 1000)]
fn bench_moffat_chi2_small(b: bench::Bencher) {
    // Small stamp: 17x17 = 289 pixels
    let (data_x, data_y, data_z) = generate_moffat_data(289, 8.5, 8.5, 1.0, 2.5, 2.5, 0.1);
    let params = [8.5f32, 8.5, 1.0, 2.5, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let chi2 = simd::compute_chi2_scalar(&data_x, &data_y, &data_z, &params, beta);
        black_box(chi2)
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            let chi2 = unsafe {
                simd::sse::compute_chi2_sse_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            let chi2 = unsafe {
                simd::avx2::compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let chi2 = unsafe {
                simd::neon::compute_chi2_neon_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }
}

#[quick_bench(warmup_iters = 50, iters = 500)]
fn bench_moffat_chi2_medium(b: bench::Bencher) {
    // Medium stamp: 25x25 = 625 pixels
    let (data_x, data_y, data_z) = generate_moffat_data(625, 12.5, 12.5, 1.0, 3.0, 2.5, 0.1);
    let params = [12.5f32, 12.5, 1.0, 3.0, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let chi2 = simd::compute_chi2_scalar(&data_x, &data_y, &data_z, &params, beta);
        black_box(chi2)
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            let chi2 = unsafe {
                simd::sse::compute_chi2_sse_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            let chi2 = unsafe {
                simd::avx2::compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let chi2 = unsafe {
                simd::neon::compute_chi2_neon_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }
}

#[quick_bench(warmup_iters = 20, iters = 200)]
fn bench_moffat_chi2_large(b: bench::Bencher) {
    // Large stamp: 33x33 = 1089 pixels
    let (data_x, data_y, data_z) = generate_moffat_data(1089, 16.5, 16.5, 1.0, 4.0, 2.5, 0.1);
    let params = [16.5f32, 16.5, 1.0, 4.0, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let chi2 = simd::compute_chi2_scalar(&data_x, &data_y, &data_z, &params, beta);
        black_box(chi2)
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            let chi2 = unsafe {
                simd::sse::compute_chi2_sse_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            let chi2 = unsafe {
                simd::avx2::compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let chi2 = unsafe {
                simd::neon::compute_chi2_neon_fixed_beta(&data_x, &data_y, &data_z, &params, beta)
            };
            black_box(chi2)
        });
    }
}

// ============================================================================
// Batch processing benchmark - 1000 stars
// ============================================================================

#[quick_bench(warmup_iters = 5, iters = 30)]
fn bench_moffat_batch_1000_jacobian(b: bench::Bencher) {
    let n_stars = 1000;
    let stamp_size = 289; // 17x17

    let stars: Vec<_> = (0..n_stars)
        .map(|i| {
            let cx = 8.0 + (i % 10) as f32 * 0.1;
            let cy = 8.0 + (i / 10 % 10) as f32 * 0.1;
            generate_moffat_data(stamp_size, cx, cy, 1.0, 2.5, 2.5, 0.1)
        })
        .collect();

    let params = [8.5f32, 8.5, 1.0, 2.5, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        for (x, y, z) in &stars {
            simd::fill_jacobian_residuals_scalar(
                x,
                y,
                z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }
        black_box(jacobian.len())
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            for (x, y, z) in &stars {
                unsafe {
                    simd::sse::fill_jacobian_residuals_sse_fixed_beta(
                        x,
                        y,
                        z,
                        &params,
                        beta,
                        &mut jacobian,
                        &mut residuals,
                    );
                }
            }
            black_box(jacobian.len())
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            for (x, y, z) in &stars {
                unsafe {
                    simd::avx2::fill_jacobian_residuals_simd_fixed_beta(
                        x,
                        y,
                        z,
                        &params,
                        beta,
                        &mut jacobian,
                        &mut residuals,
                    );
                }
            }
            black_box(jacobian.len())
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            for (x, y, z) in &stars {
                unsafe {
                    simd::neon::fill_jacobian_residuals_neon_fixed_beta(
                        x,
                        y,
                        z,
                        &params,
                        beta,
                        &mut jacobian,
                        &mut residuals,
                    );
                }
            }
            black_box(jacobian.len())
        });
    }
}

#[quick_bench(warmup_iters = 5, iters = 30)]
fn bench_moffat_batch_1000_chi2(b: bench::Bencher) {
    let n_stars = 1000;
    let stamp_size = 289; // 17x17

    let stars: Vec<_> = (0..n_stars)
        .map(|i| {
            let cx = 8.0 + (i % 10) as f32 * 0.1;
            let cy = 8.0 + (i / 10 % 10) as f32 * 0.1;
            generate_moffat_data(stamp_size, cx, cy, 1.0, 2.5, 2.5, 0.1)
        })
        .collect();

    let params = [8.5f32, 8.5, 1.0, 2.5, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let mut total = 0.0f32;
        for (x, y, z) in &stars {
            total += simd::compute_chi2_scalar(x, y, z, &params, beta);
        }
        black_box(total)
    });

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_sse4_1() {
        b.bench_labeled("sse4", || {
            let mut total = 0.0f32;
            for (x, y, z) in &stars {
                total += unsafe { simd::sse::compute_chi2_sse_fixed_beta(x, y, z, &params, beta) };
            }
            black_box(total)
        });
    }

    #[cfg(target_arch = "x86_64")]
    if cpu_features::has_avx2_fma() {
        b.bench_labeled("avx2", || {
            let mut total = 0.0f32;
            for (x, y, z) in &stars {
                total +=
                    unsafe { simd::avx2::compute_chi2_simd_fixed_beta(x, y, z, &params, beta) };
            }
            black_box(total)
        });
    }

    #[cfg(target_arch = "aarch64")]
    {
        b.bench_labeled("neon", || {
            let mut total = 0.0f32;
            for (x, y, z) in &stars {
                total +=
                    unsafe { simd::neon::compute_chi2_neon_fixed_beta(x, y, z, &params, beta) };
            }
            black_box(total)
        });
    }
}
