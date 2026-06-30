//! Benchmarks for the CPU image transform (rotation, nearest vs bilinear),
//! swept across all nine pixel formats on a ~6K (25 MP) frame.

use std::f32::consts::PI;
use std::hint::black_box;

use glam::Vec2;
use quickbench::quick_bench;

use super::{FilterMode, Transform};
use crate::common::color_format::ALL_FORMATS;
use crate::common::test_utils::create_test_image;
use crate::image::Image;

// 6K-class frame (~25 MP) — large enough to dominate per-call overhead and
// exercise the rayon-parallel per-row kernel at scale.
const WIDTH: usize = 6144;
const HEIGHT: usize = 4096;

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_transform_rotate(b: quickbench::Bencher) {
    let center = Vec2::new(WIDTH as f32 / 2.0, HEIGHT as f32 / 2.0);

    for &format in ALL_FORMATS {
        let input = create_test_image(format, WIDTH, HEIGHT, 0);
        let mut output = Image::new_black(input.desc).unwrap();

        for &filter in &[FilterMode::Nearest, FilterMode::Bilinear] {
            let transform = Transform::new()
                .rotate_around(PI / 6.0, center)
                .filter(filter);
            let label = format!("{format}/{filter:?}");

            b.bench_labeled(&label, || {
                transform.apply_cpu(black_box(&input), black_box(&mut output));
            });
        }
    }
}
