//! The contrast/brightness cases, over all nine formats: each row kernel on one row, and the op
//! on the standard frame on each backend.

use std::hint::black_box;
#[cfg(feature = "wgpu")]
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};

use common::Size2us;
use common::bench::{self, Kind, Scale};

use crate::bench::fixture::{self, Backend, Backends, Variant};
use crate::bench::scene::{self, FRAME};
use crate::color::color_format::ColorFormat;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;
use crate::ops::contrast_brightness::cpu::AffineKernel;
use crate::ops::contrast_brightness::{ContrastBrightness, cpu};

/// Both knobs off their identity values, so no path can short-circuit and the fused
/// `v * contrast + offset` the kernels evaluate has a nonzero offset.
const OP: ContrastBrightness = ContrastBrightness::new(1.2, 0.05);

/// The slowest GPU case's iteration as measured on a Radeon 680M (RGBA f32, 315 ms): the round
/// trip copies the frame to the device and back.
#[cfg(feature = "wgpu")]
const GPU_ITERATION: Duration = Duration::from_millis(320);

/// `atomic/contrast_brightness/cpu/<variant>/<format>`: one row of the standard frame in place,
/// through the scalar reference and the format's kernel on each tier this CPU has, on a fresh
/// copy each iteration. Per pixel, reading the row.
pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "contrast_brightness", Scale::Micros);
    for format in ColorFormat::ALL {
        let mut source = None;
        let affine = ChannelAffine::new(OP, format);
        let kernel = AffineKernel::of(format);
        let bytes = FRAME.width * format.byte_len();
        group.throughput(fixture::throughput(FRAME.width, format.byte_len()));
        for variant in Variant::all(Some(kernel)) {
            let id = BenchmarkId::new(Backend::Cpu.case(variant.id()), fixture::format_id(format));
            group.bench_function(id, |b| {
                let row = source
                    .get_or_insert_with(|| scene::sky(format, Size2us::new(FRAME.width, 1), 1));
                if let Variant::Kernel { kernel, tier } = variant {
                    b.iter_batched_ref(
                        || row.clone(),
                        |row| kernel.adjust_row(tier, affine, black_box(row.bytes_mut())),
                        bench::fresh_copies(bytes),
                    );
                } else {
                    b.iter_batched_ref(
                        || row.clone(),
                        |row| {
                            cpu::internals::scalar_row(affine, format, black_box(row.bytes_mut()));
                        },
                        bench::fresh_copies(bytes),
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
    let mut group = Kind::Integration.group(c, "contrast_brightness", Scale::Millis);
    for format in ColorFormat::ALL {
        let mut source = None;
        let bytes = FRAME.pixel_count() * format.byte_len();
        group.throughput(fixture::throughput(FRAME.pixel_count(), format.byte_len()));
        for backend in backends.all() {
            match backend {
                Backend::Cpu => Scale::Millis,
                #[cfg(feature = "wgpu")]
                Backend::Gpu => Scale::Slow {
                    iteration: GPU_ITERATION,
                },
            }
            .apply(&mut group);
            let id = BenchmarkId::new(backend.id(), fixture::format_id(format));
            group.bench_function(id, |b| {
                let frame = source.get_or_insert_with(|| scene::sky(format, FRAME, 1));
                match backend {
                    Backend::Cpu => b.iter_batched_ref(
                        || frame.clone(),
                        |frame| OP.apply_cpu(black_box(frame)),
                        bench::fresh_copies(bytes),
                    ),
                    #[cfg(feature = "wgpu")]
                    Backend::Gpu => {
                        let gpu = backends.gpu();
                        b.iter_with_large_drop(|| {
                            let mut image = GpuImage::from_image(gpu, frame).unwrap();
                            OP.apply_gpu(gpu, &mut image);
                            image.to_image(gpu).unwrap()
                        });
                    }
                }
            });
        }
    }
    group.finish();
}
