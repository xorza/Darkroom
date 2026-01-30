//! Benchmarks for convolution operations.

use super::simd::convolve_row_simd;
use super::{elliptical_gaussian_convolve, gaussian_convolve, gaussian_kernel_1d, matched_filter};
use crate::common::Buffer2;
use crate::star_detection::convolution::simd::convolve_row_scalar;
use crate::testing::synthetic::stamps::benchmark_star_field;
use ::bench::quick_bench;
use std::hint::black_box;

// ============ Row convolution: SIMD vs Scalar ============

#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_convolve_row_4k(b: ::bench::Bencher) {
    let width = 4096 * 10;
    let input: Vec<f32> = (0..width).map(|i| (i as f32 * 0.1).sin() * 100.0).collect();
    let kernel = gaussian_kernel_1d(2.0); // FWHM ~4.7 pixels
    let radius = kernel.len() / 2;
    let mut output = vec![0.0f32; width];

    b.bench_labeled("simd", || {
        convolve_row_simd(
            black_box(&input),
            black_box(&mut output),
            black_box(&kernel),
            radius,
        );
    });

    b.bench_labeled("scalar", || {
        convolve_row_scalar(
            black_box(&input),
            black_box(&mut output),
            black_box(&kernel),
            radius,
        );
    });
}

#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_convolve_row_large_kernel(b: ::bench::Bencher) {
    let width = 4096;
    let input: Vec<f32> = (0..width).map(|i| (i as f32 * 0.1).sin() * 100.0).collect();
    let kernel = gaussian_kernel_1d(5.0); // Larger kernel, FWHM ~11.8 pixels
    let radius = kernel.len() / 2;
    let mut output = vec![0.0f32; width];

    b.bench_labeled("simd", || {
        convolve_row_simd(
            black_box(&input),
            black_box(&mut output),
            black_box(&kernel),
            radius,
        );
    });

    b.bench_labeled("scalar", || {
        convolve_row_scalar(
            black_box(&input),
            black_box(&mut output),
            black_box(&kernel),
            radius,
        );
    });
}

// ============ Full image convolution ============

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_gaussian_convolve_1k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 42);
    let sigma = 2.0;

    b.bench(|| {
        black_box(gaussian_convolve(black_box(&pixels), black_box(sigma)));
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_gaussian_convolve_4k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(4096, 4096, 500, 0.1, 0.01, 42);
    let sigma = 2.0;

    b.bench(|| {
        black_box(gaussian_convolve(black_box(&pixels), black_box(sigma)));
    });
}

// ============ Elliptical convolution ============

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_elliptical_convolve_1k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 42);
    let sigma = 2.0;
    let axis_ratio = 0.7;
    let angle = 0.5;

    b.bench(|| {
        black_box(elliptical_gaussian_convolve(
            black_box(&pixels),
            black_box(sigma),
            black_box(axis_ratio),
            black_box(angle),
        ));
    });
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_elliptical_vs_circular_1k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 42);
    let sigma = 2.0;

    b.bench_labeled("circular", || {
        black_box(gaussian_convolve(black_box(&pixels), black_box(sigma)));
    });

    b.bench_labeled("elliptical_0.7", || {
        black_box(elliptical_gaussian_convolve(
            black_box(&pixels),
            black_box(sigma),
            black_box(0.7),
            black_box(0.5),
        ));
    });
}

// ============ Matched filter (full pipeline) ============

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_matched_filter_1k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 42);
    let background = Buffer2::new_filled(1024, 1024, 0.1);
    let fwhm = 4.0;

    b.bench_labeled("circular", || {
        black_box(matched_filter(
            black_box(&pixels),
            black_box(&background),
            black_box(fwhm),
            black_box(1.0),
            black_box(0.0),
        ));
    });

    b.bench_labeled("elliptical", || {
        black_box(matched_filter(
            black_box(&pixels),
            black_box(&background),
            black_box(fwhm),
            black_box(0.7),
            black_box(0.5),
        ));
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_matched_filter_4k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(4096, 4096, 500, 0.1, 0.01, 42);
    let background = Buffer2::new_filled(4096, 4096, 0.1);
    let fwhm = 4.0;

    b.bench(|| {
        black_box(matched_filter(
            black_box(&pixels),
            black_box(&background),
            black_box(fwhm),
            black_box(1.0),
            black_box(0.0),
        ));
    });
}
