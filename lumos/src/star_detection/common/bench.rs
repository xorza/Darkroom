//! Benchmarks for common star detection utilities.

use super::dilate_mask;
use crate::common::BitBuffer2;
use ::bench::quick_bench;
use std::hint::black_box;

/// Create a sparse mask with some set bits for benchmarking.
fn create_sparse_mask(width: usize, height: usize) -> BitBuffer2 {
    let mut mask = BitBuffer2::new_default(width, height);
    // Set ~1% of pixels in a scattered pattern
    for y in (0..height).step_by(10) {
        for x in (0..width).step_by(10) {
            mask.set_xy(x, y, true);
        }
    }
    mask
}

#[quick_bench(warmup_iters = 1, iters = 10)]
fn bench_dilate_mask_6k(b: ::bench::Bencher) {
    let mask = create_sparse_mask(6144, 6144);
    let mut output = BitBuffer2::new_default(6144, 6144);

    b.bench(|| {
        dilate_mask(black_box(&mask), black_box(3), black_box(&mut output));
    });
}
