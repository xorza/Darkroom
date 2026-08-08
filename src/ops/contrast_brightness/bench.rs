//! Benchmarks for the CPU contrast/brightness op, sweeping all nine pixel
//! formats at two frame sizes: the public path (a SIMD kernel on every format
//! this arch covers) against the scalar reference it replaces.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::ContrastBrightness;
use super::cpu::apply_typed;
use crate::common::color_format::{ALL_FORMATS, ChannelSize, ChannelType};
use crate::common::internals::create_test_image;
use crate::image::Image;

/// A frame size to sweep, and what it is there to expose.
#[derive(Debug, Clone, Copy)]
struct Frame {
    label: &'static str,
    width: usize,
    height: usize,
}

/// Both sizes are needed, and they answer different questions.
///
/// At 25 MP every format is pinned to the memory roof, so the numbers say what
/// a big frame costs but reveal nothing about kernel quality — a kernel twice
/// as good measures the same. At 4 MP the smaller formats sit in cache and the
/// kernels become the limit, which is the only place a compute regression, or a
/// SIMD path that has stopped paying for itself, is visible at all.
const FRAMES: [Frame; 2] = [
    Frame {
        label: "25MP",
        width: 6144,
        height: 4096,
    },
    Frame {
        label: "4MP",
        width: 2048,
        height: 2048,
    },
];

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

    for frame in FRAMES {
        for &format in ALL_FORMATS {
            // Criterion turns ids into report paths, so keep them space-free.
            let label = format!("{format}_{}", frame.label).replace(' ', "_");

            for &(variant, apply) in &[("auto", apply_auto as ApplyFn), ("scalar", apply_scalar)] {
                // A fresh image per variant so both start from identical pixels.
                // The op is in place and repeated iterations drive the data
                // toward saturation, which costs the same: the kernels are
                // branch-free and clamp with min/max, so timing is
                // data-independent.
                let mut image = create_test_image(format, frame.width, frame.height, 0);

                group.throughput(Throughput::Bytes(image.bytes().len() as u64));
                group.bench_function(BenchmarkId::new(variant, &label), |b| {
                    b.iter(|| apply(black_box(&mut image), black_box(params)));
                });
            }
        }
    }

    group.finish();
}
