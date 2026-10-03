//! Benchmarks for threshold mask creation.

use crate::bit_buffer2::BitBuffer2;
use crate::internals::prelude::*;
use crate::star_detection::threshold_mask::ThresholdParams;
use crate::star_detection::threshold_mask::simd::internals::process_words_scalar;
use crate::star_detection::threshold_mask::simd::process_words;
use ::quickbench::quick_bench;
use std::hint::black_box;

/// Residual and noise planes for one threshold pass.
#[derive(Debug)]
struct BenchData {
    pixels: Buffer2<f32>,
    noise: Buffer2<f32>,
}

#[expect(
    clippy::cast_sign_loss,
    reason = "synthetic fixtures are small images with non-negative coordinates"
)]
fn create_bench_data(size: usize) -> BenchData {
    let mut pixels_data = vec![0.0f32; size];
    let mut noise_data = vec![0.1f32; size];

    for i in 0..size {
        pixels_data[i] = ((i * 17) % 100) as f32 / 50.0;
        noise_data[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0;
    }

    let width = (size as f64).sqrt() as usize;
    let height = size / width;
    let actual_size = width * height;

    BenchData {
        pixels: Buffer2::new(width, height, pixels_data[..actual_size].to_vec()),
        noise: Buffer2::new(width, height, noise_data[..actual_size].to_vec()),
    }
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_threshold_mask_4k(b: ::quickbench::Bencher) {
    let BenchData { pixels, noise } = create_bench_data(4096 * 4096);
    let mut mask = BitBuffer2::new_filled(Size2us::new(4096, 4096), false);

    b.bench_labeled("simd", || {
        let words = &mut black_box(&mut mask).words;
        process_words(
            black_box(pixels.pixels()),
            black_box(noise.pixels()),
            black_box(ThresholdParams {
                sigma: 3.0,
                min_noise: 1e-6,
            }),
            words,
        );
    });

    b.bench_labeled("scalar", || {
        let words = &mut black_box(&mut mask).words;
        process_words_scalar(
            black_box(pixels.pixels()),
            black_box(noise.pixels()),
            black_box(ThresholdParams {
                sigma: 3.0,
                min_noise: 1e-6,
            }),
            words,
        );
    });
}
