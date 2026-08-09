//! Does the vectorized 3x3 median row kernel beat the plain scalar loop?
//!
//! This is what sets — or retires — the `*_ROW_WIDTH_CROSSOVER` constants the dispatcher gates on.
//! It filters a whole image rather than timing one row: a single row of a realistic width runs in
//! a few microseconds, short enough that timer overhead and machine state swamped the difference
//! (an earlier per-row sweep put AVX2 anywhere from 8x faster to 3x slower than scalar at the same
//! width, depending on which other widths were in the sweep). A megapixel of rows takes
//! milliseconds and is stable.
//!
//! Rows run sequentially, not through rayon: production parallelizes across rows either way, so
//! the scheduler is common to both arms and only adds variance to the thing being compared.

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::star_detection::median_filter::simd::{
    median_filter_row_scalar, median_filter_row_simd,
};

/// Frame widths from "narrow enough that the dispatcher's threshold is in play" up to a 6k sensor.
const IMAGE_WIDTHS: [usize; 5] = [16, 64, 256, 1024, 4096];

/// Rows per image, chosen with the widths above to keep each sample near a megapixel of work.
fn rows_for(width: usize) -> usize {
    (1 << 20) / width
}

/// `SIMD` picks the dispatched kernel or the scalar reference. A const generic rather than a
/// function pointer so both arms inline exactly as they do in production — a `fn` pointer would
/// block inlining of the scalar row loop and quietly hand the comparison to the vector arm.
fn filter_interior<const SIMD: bool>(input: &[f32], output: &mut [f32], width: usize, rows: usize) {
    for y in 1..rows - 1 {
        let above = &input[(y - 1) * width..y * width];
        let curr = &input[y * width..(y + 1) * width];
        let below = &input[(y + 1) * width..(y + 2) * width];
        let out = &mut output[y * width..(y + 1) * width];
        if SIMD {
            median_filter_row_simd(above, curr, below, out, width);
        } else {
            median_filter_row_scalar(above, curr, below, out, width);
        }
    }
}

#[quick_bench(warmup_iters = 2, iters = 20)]
fn bench_median_filter_dispatch_vs_scalar(b: ::quickbench::Bencher) {
    for width in IMAGE_WIDTHS {
        let rows = rows_for(width);
        // Unsorted, non-monotonic values: the sorting network is data-independent but the scalar
        // remainder around it is not.
        let input: Vec<f32> = (0..width * rows)
            .map(|i| ((i * 2654435761usize) % 65521) as f32 * 1.5e-5)
            .collect();
        let mut output = vec![0.0f32; width * rows];

        b.bench_labeled(&format!("scalar_{width}"), || {
            filter_interior::<false>(black_box(&input), black_box(&mut output), width, rows);
            black_box(&output);
        });
        b.bench_labeled(&format!("dispatch_{width}"), || {
            filter_interior::<true>(black_box(&input), black_box(&mut output), width, rows);
            black_box(&output);
        });
    }
}
