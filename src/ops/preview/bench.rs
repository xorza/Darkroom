//! Benchmark for the fused downscale→`RGBA_U8` preview op, reducing a 6K-class
//! frame (~25 MP) to a 256px thumbnail across all nine source formats.

use std::hint::black_box;

use quickbench::quick_bench;

use super::Preview;
use crate::common::color_format::ALL_FORMATS;
use crate::common::test_utils::create_test_image;

const SRC_W: usize = 6144;
const SRC_H: usize = 4096;
const PREVIEW_MAX: usize = 256;

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_preview(b: quickbench::Bencher) {
    // Fit the source into a 256px box, preserving aspect (≈ 256×171).
    let scale = PREVIEW_MAX as f32 / SRC_W.max(SRC_H) as f32;
    let dst_w = (SRC_W as f32 * scale).round() as usize;
    let dst_h = (SRC_H as f32 * scale).round() as usize;
    let preview = Preview::new(dst_w, dst_h);

    for &format in ALL_FORMATS {
        let src = create_test_image(format, SRC_W, SRC_H, 0);
        b.bench_labeled(&format!("{format}"), || {
            black_box(preview.to_rgba8(black_box(&src)));
        });
    }
}
