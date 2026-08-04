//! Benchmarks for the CPU image transform (rotation, nearest vs bilinear),
//! swept across all nine pixel formats on a ~6K (25 MP) frame.

use std::f32::consts::PI;
use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};
use glam::Vec2;

use super::{FilterMode, Transform};
use crate::common::color_format::ALL_FORMATS;
use crate::common::internals::create_test_image;
use crate::image::Image;

// 6K-class frame (~25 MP) — large enough to dominate per-call overhead and
// exercise the rayon-parallel per-row kernel at scale.
const WIDTH: usize = 6144;
const HEIGHT: usize = 4096;

pub fn bench(c: &mut Criterion) {
    let center = Vec2::new(WIDTH as f32 / 2.0, HEIGHT as f32 / 2.0);

    let mut group = c.benchmark_group("transform/rotate");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    for &format in ALL_FORMATS {
        let input = create_test_image(format, WIDTH, HEIGHT, 0);
        let mut output = Image::new_black(input.desc()).unwrap();
        // Criterion turns ids into report paths, so keep them space-free.
        let label = format.to_string().replace(' ', "_");

        group.throughput(Throughput::Bytes(input.bytes().len() as u64));
        for &filter in &[FilterMode::Nearest, FilterMode::Bilinear] {
            let transform = Transform::new()
                .rotate_around(PI / 6.0, center)
                .filter(filter);

            group.bench_function(BenchmarkId::new(format!("{filter:?}"), &label), |b| {
                b.iter(|| transform.apply_cpu(black_box(&input), black_box(&mut output)));
            });
        }
    }

    group.finish();
}
