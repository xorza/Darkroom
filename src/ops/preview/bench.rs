//! Benchmark for the fused downscale→`RGBA_U8` preview op, reducing a 6K-class
//! frame (~25 MP) to a 256px thumbnail across all nine source formats.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::Preview;
use crate::common::color_format::ALL_FORMATS;
use crate::common::internals::create_test_image;

const SRC_W: usize = 6144;
const SRC_H: usize = 4096;
const PREVIEW_MAX: usize = 256;

pub fn bench(c: &mut Criterion) {
    // Fit the source into a 256px box, preserving aspect (≈ 256×171).
    let scale = PREVIEW_MAX as f32 / SRC_W.max(SRC_H) as f32;
    let dst_w = (SRC_W as f32 * scale).round() as usize;
    let dst_h = (SRC_H as f32 * scale).round() as usize;
    let preview = Preview::new(dst_w, dst_h);

    let mut group = c.benchmark_group("preview");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    for &format in ALL_FORMATS {
        let src = create_test_image(format, SRC_W, SRC_H, 0);
        // Criterion turns ids into report paths, so keep them space-free.
        let label = format.to_string().replace(' ', "_");

        group.throughput(Throughput::Bytes(src.bytes().len() as u64));
        group.bench_function(BenchmarkId::new("to_rgba8", &label), |b| {
            b.iter(|| black_box(preview.to_rgba8(black_box(&src))));
        });
    }

    group.finish();
}
