//! The blend cases, over all nine formats: each row kernel on one row pair in every mode, and the
//! op on the standard frame on each backend.

use std::hint::black_box;
#[cfg(feature = "wgpu")]
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};
use strum::IntoEnumIterator;

use common::Size2us;
use common::bench::{Kind, Scale};

use crate::bench::fixture::{self, Backend, Backends, Variant};
use crate::bench::scene::{self, FRAME};
use crate::color::color_format::ALL_FORMATS;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;
#[cfg(feature = "wgpu")]
use crate::ops::blend::pipeline::GpuBlendPipeline;
use crate::ops::blend::{Blend, BlendMode, cpu};

/// A partial mix, so neither layer passes through unchanged.
const ALPHA: f32 = 0.5;

/// The slowest GPU case's iteration as measured on a Radeon 680M (RGBA f32, 468 ms): the round
/// trip copies the frame to the device and back.
#[cfg(feature = "wgpu")]
const GPU_ITERATION: Duration = Duration::from_millis(470);

/// `atomic/blend/cpu/<variant>/<mode>/<format>`: one row pair of the standard frame in every mode,
/// through the scalar reference and each tier's kernel. Per pixel, reading both rows.
pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "blend", Scale::Micros);
    for format in ALL_FORMATS {
        let mut layers = None;
        let mut out =
            Image::new_black(ImageDesc::new(Size2us::new(FRAME.width, 1), format)).unwrap();
        group.throughput(fixture::throughput(FRAME.width, 2 * format.byte_count()));
        for mode in BlendMode::iter() {
            let params = Blend::new(mode, ALPHA);
            for variant in Variant::all(|tier| cpu::row_kernel(tier, format)) {
                let input = format!(
                    "{}/{}",
                    fixture::variant_id(mode),
                    fixture::format_id(format)
                );
                group.bench_function(
                    BenchmarkId::new(Backend::Cpu.case(variant.name), input),
                    |b| {
                        let [src, dst] = layers.get_or_insert_with(|| {
                            [1, 2]
                                .map(|seed| scene::sky(format, Size2us::new(FRAME.width, 1), seed))
                        });
                        let (src, dst, out) = (src.bytes(), dst.bytes(), out.bytes_mut());
                        if let Some(kernel) = variant.kernel {
                            b.iter(|| unsafe {
                                kernel(
                                    black_box(src),
                                    black_box(dst),
                                    black_box(&mut *out),
                                    params,
                                );
                            });
                        } else {
                            b.iter(|| {
                                cpu::scalar_row(
                                    params,
                                    format,
                                    black_box(src),
                                    black_box(dst),
                                    out,
                                );
                            });
                        }
                    },
                );
            }
        }
    }
    group.finish();
}

/// `integration/blend/<backend>/<format>`: the op on two standard frames in the default mode,
/// which the modes' atomic cases compare against. On the CPU into a caller's output; on the GPU
/// the uploads, the op and the readback. Per pixel, reading both frames.
pub(crate) fn integration(c: &mut Criterion, backends: &Backends) {
    let mut group = Kind::Integration.group(c, "blend", Scale::Millis);
    let params = Blend::new(BlendMode::default(), ALPHA);
    #[cfg(feature = "wgpu")]
    let mut pipeline = None;
    for format in ALL_FORMATS {
        let mut layers = None;
        let pixels = FRAME.pixel_count();
        group.throughput(fixture::throughput(pixels, 2 * format.byte_count()));
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
                let [src, dst] = layers
                    .get_or_insert_with(|| [1, 2].map(|seed| scene::sky(format, FRAME, seed)));
                match backend {
                    Backend::Cpu => {
                        let mut output = Image::new_black(src.desc()).unwrap();
                        b.iter(|| params.apply_cpu(black_box(src), black_box(dst), &mut output));
                    }
                    #[cfg(feature = "wgpu")]
                    Backend::Gpu => {
                        let gpu = backends.gpu();
                        let pipeline = pipeline.get_or_insert_with(|| GpuBlendPipeline::new(gpu));
                        b.iter_with_large_drop(|| {
                            let top = GpuImage::from_image(gpu, src).unwrap();
                            let bottom = GpuImage::from_image(gpu, dst).unwrap();
                            let mut output = GpuImage::new_empty(gpu, src.desc()).unwrap();
                            params.apply_gpu(gpu, pipeline, &top, &bottom, &mut output);
                            output.to_image(gpu).unwrap()
                        });
                    }
                }
            });
        }
    }
    group.finish();
}
