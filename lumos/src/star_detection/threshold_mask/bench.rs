//! Benchmarks for threshold mask creation.

use super::{
    process_words, process_words_adaptive, process_words_adaptive_scalar, process_words_filtered,
    process_words_filtered_scalar, process_words_scalar,
};
use crate::common::{BitBuffer2, Buffer2};
use ::bench::quick_bench;
use std::hint::black_box;

fn create_bench_data(size: usize) -> (Buffer2<f32>, Buffer2<f32>, Buffer2<f32>, Buffer2<f32>) {
    let mut pixels_data = vec![0.0f32; size];
    let mut bg_data = vec![1.0f32; size];
    let mut noise_data = vec![0.1f32; size];
    let mut adaptive_sigma_data = vec![3.0f32; size];

    for i in 0..size {
        pixels_data[i] = ((i * 17) % 100) as f32 / 50.0;
        bg_data[i] = 1.0 + ((i * 7) % 10) as f32 / 100.0;
        noise_data[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0;
        adaptive_sigma_data[i] = 3.0 + ((i * 11) % 20) as f32 / 10.0; // Varies 3.0-5.0
    }

    let width = (size as f64).sqrt() as usize;
    let height = size / width;
    let actual_size = width * height;

    (
        Buffer2::new(width, height, pixels_data[..actual_size].to_vec()),
        Buffer2::new(width, height, bg_data[..actual_size].to_vec()),
        Buffer2::new(width, height, noise_data[..actual_size].to_vec()),
        Buffer2::new(width, height, adaptive_sigma_data[..actual_size].to_vec()),
    )
}

#[quick_bench(warmup_iters = 3, iters = 200)]
fn bench_threshold_mask_4k(b: ::bench::Bencher) {
    let (pixels, bg, noise, adaptive_sigma) = create_bench_data(4096 * 4096);
    let mut mask = BitBuffer2::new_filled(4096, 4096, false);
    let total_pixels = pixels.len();

    b.bench_labeled("simd", || {
        let words = black_box(&mut mask).words_mut();
        process_words(
            black_box(&pixels),
            black_box(&bg),
            black_box(&noise),
            black_box(3.0),
            words,
            0,
            total_pixels,
        );
    });

    b.bench_labeled("scalar", || {
        let words = black_box(&mut mask).words_mut();
        process_words_scalar(
            black_box(pixels.pixels()),
            black_box(bg.pixels()),
            black_box(noise.pixels()),
            black_box(3.0),
            words,
            0,
            total_pixels,
        );
    });

    b.bench_labeled("filtered_simd", || {
        let words = black_box(&mut mask).words_mut();
        process_words_filtered(
            black_box(&pixels),
            black_box(&noise),
            black_box(3.0),
            words,
            0,
            total_pixels,
        );
    });

    b.bench_labeled("filtered_scalar", || {
        let words = black_box(&mut mask).words_mut();
        process_words_filtered_scalar(
            black_box(pixels.pixels()),
            black_box(noise.pixels()),
            black_box(3.0),
            words,
            0,
            total_pixels,
        );
    });

    b.bench_labeled("adaptive_simd", || {
        let words = black_box(&mut mask).words_mut();
        process_words_adaptive(
            black_box(&pixels),
            black_box(&bg),
            black_box(&noise),
            black_box(&adaptive_sigma),
            words,
            0,
            total_pixels,
        );
    });

    b.bench_labeled("adaptive_scalar", || {
        let words = black_box(&mut mask).words_mut();
        process_words_adaptive_scalar(
            black_box(pixels.pixels()),
            black_box(bg.pixels()),
            black_box(noise.pixels()),
            black_box(adaptive_sigma.pixels()),
            words,
            0,
            total_pixels,
        );
    });
}
