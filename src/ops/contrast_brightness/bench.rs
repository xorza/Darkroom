//! Benchmarks for the CPU contrast/brightness op on a ~6K (25 MP) frame, swept
//! across all nine pixel formats: the public path (a SIMD kernel on every
//! format this arch covers) against the scalar reference it replaces.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::ContrastBrightness;
use super::cpu::apply_typed;
use crate::common::color_format::{ALL_FORMATS, ChannelSize, ChannelType};
use crate::common::internals::create_test_image;
use crate::image::Image;
use crate::processing_context::ProcessingContext;
use crate::processing_context::image_buffer::ImageBuffer;

// 6K-class frame (~25 MP) — large enough to dominate per-call overhead and
// exercise the rayon-parallel per-row kernel at scale.
const WIDTH: usize = 6144;
const HEIGHT: usize = 4096;

/// Both knobs off their identity values, so neither path can short-circuit and
/// the fused `v * contrast + offset` form the SIMD kernels use is exercised
/// with a non-trivial offset.
const CONTRAST: f32 = 1.2;
const BRIGHTNESS: f32 = 0.05;

/// A benchmarked variant: the public entry point, or the scalar reference.
type ApplyFn = fn(&mut Image, ContrastBrightness);

fn apply_auto(image: &mut Image, params: ContrastBrightness) {
    params.apply_cpu(image);
}

/// The scalar reference, dispatched on storage type — the same dispatch
/// `cpu::apply` falls back to when no SIMD kernel matches the format.
fn apply_scalar(image: &mut Image, params: ContrastBrightness) {
    match (
        image.desc().color_format.channel_size,
        image.desc().color_format.channel_type,
    ) {
        (ChannelSize::_8bit, ChannelType::UInt) => apply_typed::<u8>(image, params),
        (ChannelSize::_16bit, ChannelType::UInt) => apply_typed::<u16>(image, params),
        (ChannelSize::_32bit, ChannelType::Float) => apply_typed::<f32>(image, params),
        _ => unreachable!("unsupported format in ALL_FORMATS"),
    }
}

pub fn bench(c: &mut Criterion) {
    let params = ContrastBrightness::new(CONTRAST, BRIGHTNESS);

    let mut group = c.benchmark_group("contrast_brightness");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    for &format in ALL_FORMATS {
        // Criterion turns ids into report paths, so keep them space-free.
        let label = format.to_string().replace(' ', "_");

        for &(variant, apply) in &[("auto", apply_auto as ApplyFn), ("scalar", apply_scalar)] {
            // A fresh image per variant so both start from identical pixels.
            // The op is in place and repeated iterations drive the data toward
            // saturation, which costs the same: the kernels are branch-free and
            // clamp with min/max, so timing is data-independent.
            let mut image = create_test_image(format, WIDTH, HEIGHT, 0);

            group.throughput(Throughput::Bytes(image.bytes().len() as u64));
            group.bench_function(BenchmarkId::new(variant, &label), |b| {
                b.iter(|| apply(black_box(&mut image), black_box(params)));
            });
        }
    }

    group.finish();

    bench_execute(c);
}

/// The buffer-level entry point a pipeline calls.
///
/// Benched because this path is where the op's cost is easiest to lose — it
/// once spent an order of magnitude more time faulting in a fresh output
/// allocation than adjusting the pixels.
fn bench_execute(c: &mut Criterion) {
    let params = ContrastBrightness::new(CONTRAST, BRIGHTNESS);

    let mut group = c.benchmark_group("contrast_brightness_execute");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    for &format in ALL_FORMATS {
        let label = format.to_string().replace(' ', "_");
        let mut ctx = ProcessingContext::cpu_only();
        let source = create_test_image(format, WIDTH, HEIGHT, 0);
        let mut buffer = ImageBuffer::from_cpu(source.clone());

        group.throughput(Throughput::Bytes(source.bytes().len() as u64));
        group.bench_function(BenchmarkId::new("execute", &label), |b| {
            b.iter(|| params.execute(&mut ctx, black_box(&mut buffer)).unwrap());
        });
    }

    group.finish();
}
