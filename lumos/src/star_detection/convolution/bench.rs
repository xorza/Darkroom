//! Benchmarks for convolution operations.

use crate::internals::prelude::*;
use crate::internals::synthetic::fixtures::star_field;
use crate::star_detection::config::fwhm_config::MatchedFilter;
use crate::star_detection::convolution::internals::{
    elliptical_gaussian_convolve, gaussian_convolve, gaussian_kernel_1d, matched_filter_fresh,
};
use crate::star_detection::convolution::simd::convolve_row;
use crate::star_detection::convolution::simd::internals::convolve_row_scalar;
use crate::star_detection::convolution::{convolve_cols, convolve_rows_parallel};
use ::quickbench::quick_bench;
use std::hint::black_box;

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_convolve_row_4k(b: ::quickbench::Bencher) {
    let width = 4096 * 10;
    let input: Vec<f32> = (0..width).map(|i| (i as f32 * 0.1).sin() * 100.0).collect();
    let kernel = gaussian_kernel_1d(2.0); // FWHM ~4.7 pixels
    let mut output = vec![0.0f32; width];

    b.bench_labeled("simd", || {
        convolve_row(
            black_box(&input),
            black_box(&mut output),
            black_box(&kernel),
        );
    });

    b.bench_labeled("scalar", || {
        convolve_row_scalar(
            black_box(&input),
            black_box(&mut output),
            black_box(&kernel),
        );
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_convolve_row_large_kernel(b: ::quickbench::Bencher) {
    let width = 4096;
    let input: Vec<f32> = (0..width).map(|i| (i as f32 * 0.1).sin() * 100.0).collect();
    let kernel = gaussian_kernel_1d(5.0); // Larger kernel, FWHM ~11.8 pixels
    let mut output = vec![0.0f32; width];

    b.bench_labeled("simd", || {
        convolve_row(
            black_box(&input),
            black_box(&mut output),
            black_box(&kernel),
        );
    });

    b.bench_labeled("scalar", || {
        convolve_row_scalar(
            black_box(&input),
            black_box(&mut output),
            black_box(&kernel),
        );
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_convolve_cols_1k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(1024, 1024), 100, 42)
        .image
        .channel(0)
        .clone();
    let kernel = gaussian_kernel_1d(2.0);
    let mut output = Buffer2::new_default(1024, 1024);

    b.bench(|| {
        convolve_cols(
            black_box(&pixels),
            black_box(&mut output),
            black_box(&kernel),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_convolve_cols_4k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(4096, 4096), 500, 42)
        .image
        .channel(0)
        .clone();
    let kernel = gaussian_kernel_1d(2.0);
    let mut output = Buffer2::new_default(4096, 4096);

    b.bench(|| {
        convolve_cols(
            black_box(&pixels),
            black_box(&mut output),
            black_box(&kernel),
        );
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_row_vs_col_1k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(1024, 1024), 100, 42)
        .image
        .channel(0)
        .clone();
    let kernel = gaussian_kernel_1d(2.0);
    let mut output = Buffer2::new_default(1024, 1024);

    b.bench_labeled("rows", || {
        convolve_rows_parallel(
            black_box(&pixels),
            black_box(&mut output),
            black_box(&kernel),
        );
    });

    b.bench_labeled("cols", || {
        convolve_cols(
            black_box(&pixels),
            black_box(&mut output),
            black_box(&kernel),
        );
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_gaussian_convolve_1k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(1024, 1024), 100, 42)
        .image
        .channel(0)
        .clone();
    let mut output = Buffer2::new_default(1024, 1024);
    let mut temp = Buffer2::new_default(1024, 1024);
    let sigma = 2.0;

    b.bench(|| {
        output.pixels_mut().copy_from_slice(pixels.pixels());
        gaussian_convolve(
            black_box(&mut output),
            black_box(sigma),
            black_box(&mut temp),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_gaussian_convolve_4k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(4096, 4096), 500, 42)
        .image
        .channel(0)
        .clone();
    let mut output = Buffer2::new_default(4096, 4096);
    let mut temp = Buffer2::new_default(4096, 4096);
    let sigma = 2.0;

    b.bench(|| {
        output.pixels_mut().copy_from_slice(pixels.pixels());
        gaussian_convolve(
            black_box(&mut output),
            black_box(sigma),
            black_box(&mut temp),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_elliptical_convolve_1k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(1024, 1024), 100, 42)
        .image
        .channel(0)
        .clone();
    let mut output = Buffer2::new_default(1024, 1024);
    let sigma = 2.0;
    let axis_ratio = 0.7;
    let angle = 0.5;

    b.bench(|| {
        elliptical_gaussian_convolve(
            black_box(&pixels),
            black_box(sigma),
            black_box(axis_ratio),
            black_box(angle),
            black_box(&mut output),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_elliptical_vs_circular_1k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(1024, 1024), 100, 42)
        .image
        .channel(0)
        .clone();
    let mut output = Buffer2::new_default(1024, 1024);
    let mut temp = Buffer2::new_default(1024, 1024);
    let sigma = 2.0;

    b.bench_labeled("circular", || {
        output.pixels_mut().copy_from_slice(pixels.pixels());
        gaussian_convolve(
            black_box(&mut output),
            black_box(sigma),
            black_box(&mut temp),
        );
    });

    b.bench_labeled("elliptical_0.7", || {
        elliptical_gaussian_convolve(
            black_box(&pixels),
            black_box(sigma),
            black_box(0.7),
            black_box(0.5),
            black_box(&mut output),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_matched_filter_1k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(1024, 1024), 100, 42)
        .image
        .channel(0)
        .clone();
    let mut output = Buffer2::new_default(1024, 1024);
    let mut temp = Buffer2::new_default(1024, 1024);
    let fwhm = 4.0;

    b.bench_labeled("circular", || {
        output.pixels_mut().copy_from_slice(pixels.pixels());
        matched_filter_fresh(
            black_box(&mut output),
            black_box(MatchedFilter {
                fwhm,
                axis_ratio: 1.0,
                angle: 0.0,
            }),
            black_box(&mut temp),
        );
    });

    for (label, angle) in [("elliptical", 0.5), ("elliptical_on_axis", 0.0)] {
        b.bench_labeled(label, || {
            output.pixels_mut().copy_from_slice(pixels.pixels());
            matched_filter_fresh(
                black_box(&mut output),
                black_box(MatchedFilter {
                    fwhm,
                    axis_ratio: 0.7,
                    angle,
                }),
                black_box(&mut temp),
            );
        });
    }
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_matched_filter_4k(b: ::quickbench::Bencher) {
    let pixels = star_field(Size2us::new(4096, 4096), 500, 42)
        .image
        .channel(0)
        .clone();
    let mut output = Buffer2::new_default(4096, 4096);
    let mut temp = Buffer2::new_default(4096, 4096);
    let fwhm = 4.0;

    b.bench(|| {
        output.pixels_mut().copy_from_slice(pixels.pixels());
        matched_filter_fresh(
            black_box(&mut output),
            black_box(MatchedFilter {
                fwhm,
                axis_ratio: 1.0,
                angle: 0.0,
            }),
            black_box(&mut temp),
        );
    });
}
