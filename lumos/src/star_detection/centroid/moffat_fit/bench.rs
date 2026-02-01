//! Benchmarks for Moffat SIMD vs Scalar comparison.
//!
//! Run with: `cargo test -p lumos --release bench_moffat_simd -- --ignored --nocapture`

use bench::quick_bench;
use std::hint::black_box;

use super::super::lm_optimizer::{LMConfig, LMResult, optimize_5};
use super::MoffatFixedBeta;
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

/// Scalar-only L-M optimizer for Moffat with fixed beta (for comparison).
fn optimize_moffat_scalar(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 5],
    beta: f32,
    stamp_radius: f32,
    config: &LMConfig,
) -> LMResult<5> {
    let model = MoffatFixedBeta { beta, stamp_radius };
    optimize_5(&model, data_x, data_y, data_z, initial_params, config)
}

// ============================================================================
// Single fit benchmarks
// ============================================================================

#[quick_bench(warmup_iters = 50, iters = 500)]
fn bench_moffat_simd_vs_scalar_small(b: bench::Bencher) {
    // Small stamp: 17x17 = 289 pixels (typical star stamp)
    let (data_x, data_y, data_z) = generate_moffat_data(289, 8.5, 8.5, 1.0, 2.5, 2.5, 0.1);
    let initial_params = [8.0f32, 8.0, 0.9, 2.0, 0.15];
    let beta = 2.5f32;
    let stamp_radius = 8.0f32;
    let config = LMConfig::default();

    b.bench_labeled("scalar", || {
        let result = optimize_moffat_scalar(
            &data_x,
            &data_y,
            &data_z,
            initial_params,
            beta,
            stamp_radius,
            &config,
        );
        black_box(result)
    });

    b.bench_labeled("simd", || {
        let result = super::optimize_moffat_fixed_beta_simd(
            &data_x,
            &data_y,
            &data_z,
            initial_params,
            beta,
            stamp_radius,
            &config,
        );
        black_box(result)
    });
}

#[quick_bench(warmup_iters = 20, iters = 200)]
fn bench_moffat_simd_vs_scalar_medium(b: bench::Bencher) {
    // Medium stamp: 25x25 = 625 pixels
    let (data_x, data_y, data_z) = generate_moffat_data(625, 12.5, 12.5, 1.0, 3.0, 2.5, 0.1);
    let initial_params = [12.0f32, 12.0, 0.9, 2.5, 0.15];
    let beta = 2.5f32;
    let stamp_radius = 12.0f32;
    let config = LMConfig::default();

    b.bench_labeled("scalar", || {
        let result = optimize_moffat_scalar(
            &data_x,
            &data_y,
            &data_z,
            initial_params,
            beta,
            stamp_radius,
            &config,
        );
        black_box(result)
    });

    b.bench_labeled("simd", || {
        let result = super::optimize_moffat_fixed_beta_simd(
            &data_x,
            &data_y,
            &data_z,
            initial_params,
            beta,
            stamp_radius,
            &config,
        );
        black_box(result)
    });
}

#[quick_bench(warmup_iters = 10, iters = 100)]
fn bench_moffat_simd_vs_scalar_large(b: bench::Bencher) {
    // Large stamp: 33x33 = 1089 pixels
    let (data_x, data_y, data_z) = generate_moffat_data(1089, 16.5, 16.5, 1.0, 4.0, 2.5, 0.1);
    let initial_params = [16.0f32, 16.0, 0.9, 3.5, 0.15];
    let beta = 2.5f32;
    let stamp_radius = 16.0f32;
    let config = LMConfig::default();

    b.bench_labeled("scalar", || {
        let result = optimize_moffat_scalar(
            &data_x,
            &data_y,
            &data_z,
            initial_params,
            beta,
            stamp_radius,
            &config,
        );
        black_box(result)
    });

    b.bench_labeled("simd", || {
        let result = super::optimize_moffat_fixed_beta_simd(
            &data_x,
            &data_y,
            &data_z,
            initial_params,
            beta,
            stamp_radius,
            &config,
        );
        black_box(result)
    });
}

// ============================================================================
// Jacobian-only benchmarks (isolate SIMD benefit)
// ============================================================================

#[quick_bench(warmup_iters = 100, iters = 1000)]
fn bench_jacobian_simd_vs_scalar(b: bench::Bencher) {
    // Benchmark just the Jacobian/residual computation
    let (data_x, data_y, data_z) = generate_moffat_data(289, 8.5, 8.5, 1.0, 2.5, 2.5, 0.1);
    let params = [8.5f32, 8.5, 1.0, 2.5, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let mut jacobian = Vec::with_capacity(data_x.len());
        let mut residuals = Vec::with_capacity(data_x.len());

        let alpha2 = params[3] * params[3];
        for i in 0..data_x.len() {
            let x = data_x[i];
            let y = data_y[i];
            let z = data_z[i];

            let dx = x - params[0];
            let dy = y - params[1];
            let r2 = dx * dx + dy * dy;
            let u = 1.0 + r2 / alpha2;
            let u_neg_beta = u.powf(-beta);
            let u_neg_beta_m1 = u_neg_beta / u;
            let model = params[2] * u_neg_beta + params[4];

            residuals.push(z - model);

            let common = 2.0 * params[2] * beta / alpha2 * u_neg_beta_m1;
            jacobian.push([
                common * dx,
                common * dy,
                u_neg_beta,
                common * r2 / params[3],
                1.0,
            ]);
        }
        black_box(jacobian.len() + residuals.len())
    });

    b.bench_labeled("simd", || {
        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();

        if simd::is_avx2_available() {
            unsafe {
                simd::fill_jacobian_residuals_simd_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }
        }
        black_box(jacobian.len() + residuals.len())
    });
}

#[quick_bench(warmup_iters = 100, iters = 1000)]
fn bench_chi2_simd_vs_scalar(b: bench::Bencher) {
    // Benchmark just the chi² computation
    let (data_x, data_y, data_z) = generate_moffat_data(289, 8.5, 8.5, 1.0, 2.5, 2.5, 0.1);
    let params = [8.5f32, 8.5, 1.0, 2.5, 0.1];
    let beta = 2.5f32;

    b.bench_labeled("scalar", || {
        let alpha2 = params[3] * params[3];
        let mut chi2 = 0.0f32;
        for i in 0..data_x.len() {
            let x = data_x[i];
            let y = data_y[i];
            let z = data_z[i];

            let dx = x - params[0];
            let dy = y - params[1];
            let r2 = dx * dx + dy * dy;
            let u = 1.0 + r2 / alpha2;
            let model = params[2] * u.powf(-beta) + params[4];
            let residual = z - model;
            chi2 += residual * residual;
        }
        black_box(chi2)
    });

    b.bench_labeled("simd", || {
        let chi2 = if simd::is_avx2_available() {
            unsafe { simd::compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) }
        } else {
            0.0
        };
        black_box(chi2)
    });
}

// ============================================================================
// Batch processing benchmark
// ============================================================================

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_moffat_batch_1000_stars(b: bench::Bencher) {
    // Simulate fitting 1000 stars
    let n_stars = 1000;
    let stamp_size = 289; // 17x17

    // Pre-generate all star data
    let stars: Vec<_> = (0..n_stars)
        .map(|i| {
            let cx = 8.0 + (i % 10) as f32 * 0.1;
            let cy = 8.0 + (i / 10 % 10) as f32 * 0.1;
            generate_moffat_data(stamp_size, cx, cy, 1.0, 2.5, 2.5, 0.1)
        })
        .collect();

    let initial_params = [8.0f32, 8.0, 0.9, 2.0, 0.15];
    let beta = 2.5f32;
    let stamp_radius = 8.0f32;
    let config = LMConfig::default();

    b.bench_labeled("scalar", || {
        let results: Vec<_> = stars
            .iter()
            .map(|(x, y, z)| {
                optimize_moffat_scalar(x, y, z, initial_params, beta, stamp_radius, &config)
            })
            .collect();
        black_box(results.len())
    });

    b.bench_labeled("simd", || {
        let results: Vec<_> = stars
            .iter()
            .map(|(x, y, z)| {
                super::optimize_moffat_fixed_beta_simd(
                    x,
                    y,
                    z,
                    initial_params,
                    beta,
                    stamp_radius,
                    &config,
                )
            })
            .collect();
        black_box(results.len())
    });
}
