//! Benchmarks for threshold mask creation.

use super::create_threshold_mask;
use crate::common::BitBuffer2;
use ::bench::quick_bench;
use std::hint::black_box;

fn create_bench_data(size: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
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

    (
        pixels_data[..actual_size].to_vec(),
        bg[..actual_size].to_vec(),
        noise[..actual_size].to_vec(),
    )
}

#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_threshold_mask_4k(b: ::bench::Bencher) {
    let (pixels, bg, noise) = create_bench_data(4096 * 4096);
    let mut packed_mask = BitBuffer2::new_filled(4096, 4096, false);

    b.bench_labeled("packed", || {
        create_threshold_mask(
            black_box(&pixels),
            black_box(&bg),
            black_box(&noise),
            black_box(3.0),
            black_box(&mut packed_mask),
        );
    });
}
