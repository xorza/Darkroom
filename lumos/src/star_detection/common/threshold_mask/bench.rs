//! Benchmarks for threshold mask creation.

use super::*;
use ::bench::quick_bench;
use std::hint::black_box;

fn create_bench_data(size: usize) -> (Buffer2<f32>, BackgroundMap, Buffer2<bool>) {
    let mut pixels_data = vec![0.0f32; size];
    let mut bg = vec![1.0f32; size];
    let mut noise = vec![0.1f32; size];

    for i in 0..size {
        pixels_data[i] = ((i * 17) % 100) as f32 / 50.0;
        bg[i] = 1.0 + ((i * 7) % 10) as f32 / 100.0;
        noise[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0;
    }

    let width = (size as f64).sqrt() as usize;
    let height = size / width;
    let actual_size = width * height;

    let pixels = Buffer2::new(width, height, pixels_data[..actual_size].to_vec());
    let background = BackgroundMap {
        background: Buffer2::new(width, height, bg[..actual_size].to_vec()),
        noise: Buffer2::new(width, height, noise[..actual_size].to_vec()),
    };
    let mask = Buffer2::new_filled(width, height, false);

    (pixels, background, mask)
}

#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_threshold_mask_4k(b: ::bench::Bencher) {
    let (pixels, background, mut mask) = create_bench_data(4096 * 4096);

    b.bench_labeled("scalar", || {
        scalar::process_chunk_scalar::<true>(
            black_box(pixels.pixels()),
            black_box(background.background.pixels()),
            black_box(background.noise.pixels()),
            black_box(3.0),
            black_box(mask.pixels_mut()),
        );
    });

    b.bench_labeled("simd", || {
        process_chunk_simd::<true>(
            black_box(pixels.pixels()),
            black_box(background.background.pixels()),
            black_box(background.noise.pixels()),
            black_box(3.0),
            black_box(mask.pixels_mut()),
        );
    });
}
