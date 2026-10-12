//! The contrast/brightness cases, over all nine formats: each row kernel on one row, and the op
//! on the standard frame on each backend.

use std::hint::black_box;
#[cfg(feature = "wgpu")]
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};

use crate::bench::fixture::{self, Backend, Backends, Kind, Scale, Variant};
use crate::bench::scene::{self, HEIGHT, WIDTH};
use crate::common::color_format::ALL_FORMATS;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;
#[cfg(feature = "wgpu")]
use crate::ops::contrast_brightness::pipeline::GpuContrastBrightnessPipeline;
use crate::ops::contrast_brightness::{ContrastBrightness, cpu};

/// Both knobs off their identity values, so no path can short-circuit and the fused
/// `v * contrast + offset` the kernels evaluate has a nonzero offset.
const OP: ContrastBrightness = ContrastBrightness {
    contrast: 1.2,
    brightness: 0.05,
};

/// The slowest GPU case's iteration as measured on a Radeon 680M (RGBA f32, 315 ms): the round
/// trip copies the frame to the device and back.
#[cfg(feature = "wgpu")]
const GPU_ITERATION: Duration = Duration::from_millis(320);

/// `atomic/contrast_brightness/<variant>/<format>`: one row of the standard frame in place,
/// through the scalar reference and each tier's kernel, on a fresh copy each iteration. Per
/// pixel, reading the row.
pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "contrast_brightness", Scale::Sample);
    for format in ALL_FORMATS {
        let mut source = None;
        let affine = ChannelAffine::new(OP, format);
        let bytes = WIDTH * format.byte_count();
        group.throughput(fixture::throughput(WIDTH, format.byte_count()));
        for variant in Variant::all(|tier| cpu::row_kernel(tier, format)) {
            let id = BenchmarkId::new(variant.name, fixture::format_id(format));
            group.bench_function(id, |b| {
                let row = source.get_or_insert_with(|| scene::sky(format, WIDTH, 1, 1));
                if let Some(kernel) = variant.kernel {
                    let count = cpu::kernel_count(row.desc());
                    b.iter_batched_ref(
                        || row.clone(),
                        // SAFETY: a variant holds only kernels of tiers this CPU has.
                        |row| unsafe { kernel(black_box(row.bytes_mut()), count, affine) },
                        fixture::fresh_copies(bytes),
                    );
                } else {
                    b.iter_batched_ref(
                        || row.clone(),
                        |row| cpu::scalar_row(affine, format, black_box(row.bytes_mut())),
                        fixture::fresh_copies(bytes),
                    );
                }
            });
        }
    }
    group.finish();
}

/// `integration/contrast_brightness/<backend>/<format>`: the op on the standard frame. On the
/// CPU in place on a fresh copy each iteration; on the GPU the upload, the op and the readback.
/// Per pixel, reading the frame.
pub(crate) fn integration(c: &mut Criterion, backends: &Backends) {
    let mut group = Kind::Integration.group(c, "contrast_brightness", Scale::Frame);
    #[cfg(feature = "wgpu")]
    let mut pipeline = None;
    for format in ALL_FORMATS {
        let mut source = None;
        let bytes = WIDTH * HEIGHT * format.byte_count();
        group.throughput(fixture::throughput(WIDTH * HEIGHT, format.byte_count()));
        for backend in backends.all() {
            match backend {
                Backend::Cpu => Scale::Frame,
                #[cfg(feature = "wgpu")]
                Backend::Gpu => Scale::Set {
                    iteration: GPU_ITERATION,
                },
            }
            .apply(&mut group);
            let id = BenchmarkId::new(backend.id(), fixture::format_id(format));
            group.bench_function(id, |b| {
                let frame = source.get_or_insert_with(|| scene::sky(format, WIDTH, HEIGHT, 1));
                match backend {
                    Backend::Cpu => b.iter_batched_ref(
                        || frame.clone(),
                        |frame| OP.apply_cpu(black_box(frame)),
                        fixture::fresh_copies(bytes),
                    ),
                    #[cfg(feature = "wgpu")]
                    Backend::Gpu => {
                        let gpu = backends.gpu();
                        let pipeline =
                            pipeline.get_or_insert_with(|| GpuContrastBrightnessPipeline::new(gpu));
                        b.iter_with_large_drop(|| {
                            let input = GpuImage::from_image(gpu, frame).unwrap();
                            let mut output = GpuImage::new_empty(gpu, frame.desc()).unwrap();
                            OP.apply_gpu(gpu, pipeline, &input, &mut output);
                            output.to_image(gpu).unwrap()
                        });
                    }
                }
            });
        }
    }
    group.finish();
}
