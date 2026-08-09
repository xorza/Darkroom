//! Row-width crossover sweep for the 3x3 median filter backends.
//!
//! Sets the `*_ROW_WIDTH_CROSSOVER` constants the dispatcher gates on. Each backend vectorizes
//! only the interior `width - 2` pixels as `(width - 2) / LANES` chunks and finishes the rest
//! scalar, so a narrow row is nearly all remainder and the vector setup does not pay for itself.
//! The sweep runs one interior row at a time — the unit the dispatcher actually chooses for.

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::star_detection::median_filter::simd::median_filter_row_scalar;

/// Straddles the 8-lane AVX2 and 4-lane SSE/NEON widths from "one chunk plus remainder" up to
/// "remainder is noise", which is where the crossover has to be.
const ROW_WIDTHS: [usize; 10] = [6, 8, 10, 12, 14, 16, 24, 32, 64, 256];

/// Rows per timed sample, so a 6-wide row and a 256-wide row do comparable work per sample.
fn rows_per_sample(width: usize) -> usize {
    (16_384 / width).clamp(1, 4_096)
}

struct Rows {
    above: Vec<f32>,
    curr: Vec<f32>,
    below: Vec<f32>,
    out: Vec<f32>,
}

impl Rows {
    /// Three rows of unsorted, non-monotonic values — a sorting network is data-independent, but
    /// the surrounding branchy scalar remainder is not.
    fn new(width: usize) -> Self {
        Self {
            above: (0..width).map(|i| ((i * 37) % 101) as f32 * 0.01).collect(),
            curr: (0..width).map(|i| ((i * 53) % 101) as f32 * 0.01).collect(),
            below: (0..width).map(|i| ((i * 71) % 101) as f32 * 0.01).collect(),
            out: vec![0.0; width],
        }
    }
}

#[quick_bench(warmup_iters = 10, iters = 100)]
fn bench_median_filter_row_crossover(b: ::quickbench::Bencher) {
    for width in ROW_WIDTHS {
        let mut rows = Rows::new(width);
        let calls = rows_per_sample(width);
        // `ROW_WIDTHS` is a const array, so without this the sweep loop unrolls and each backend
        // gets a compile-time width — fully unrolled scalar code that production never runs.
        let width = black_box(width);

        b.bench_labeled(&format!("scalar_{width}"), || {
            for _ in 0..calls {
                median_filter_row_scalar(
                    black_box(&rows.above),
                    black_box(&rows.curr),
                    black_box(&rows.below),
                    black_box(&mut rows.out),
                    width,
                );
            }
        });

        #[cfg(target_arch = "x86_64")]
        {
            use crate::stacking::star_detection::median_filter::simd::x86;

            if imaginarium::cpu_features::has_avx2() {
                b.bench_labeled(&format!("avx2_{width}"), || {
                    for _ in 0..calls {
                        // SAFETY: AVX2 availability checked above.
                        unsafe {
                            x86::median_filter_row_avx2(
                                black_box(&rows.above),
                                black_box(&rows.curr),
                                black_box(&rows.below),
                                black_box(&mut rows.out),
                                width,
                            );
                        }
                    }
                });
            }
            if imaginarium::cpu_features::has_sse4_1() {
                b.bench_labeled(&format!("sse41_{width}"), || {
                    for _ in 0..calls {
                        // SAFETY: SSE4.1 availability checked above.
                        unsafe {
                            x86::median_filter_row_sse41(
                                black_box(&rows.above),
                                black_box(&rows.curr),
                                black_box(&rows.below),
                                black_box(&mut rows.out),
                                width,
                            );
                        }
                    }
                });
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            use crate::stacking::star_detection::median_filter::simd::neon;

            b.bench_labeled(&format!("neon_{width}"), || {
                for _ in 0..calls {
                    // SAFETY: NEON is unconditionally available on aarch64.
                    unsafe {
                        neon::median_filter_row_neon(
                            black_box(&rows.above),
                            black_box(&rows.curr),
                            black_box(&rows.below),
                            black_box(&mut rows.out),
                            width,
                        );
                    }
                }
            });
        }
    }
}
