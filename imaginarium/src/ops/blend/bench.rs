//! The blend cases, over all nine formats: each row kernel on one row pair in every mode, and the
//! op on the standard frame on each backend.

use std::hint::black_box;
#[cfg(feature = "wgpu")]
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};
use strum::IntoEnumIterator;

use common::USize2;
use common::bench::{Kind, Scale};

use crate::bench::fixture::{self, Backend, Backends, Variant};
use crate::bench::scene::{self, FRAME};
use crate::color::color_format::ColorFormat;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::ops::blend::cpu::BlendKernel;
use crate::ops::blend::{Blend, BlendMode, cpu};

/// A partial mix, so neither layer passes through unchanged.
const ALPHA: f32 = 0.5;

/// The slowest GPU case's iteration as measured on a Radeon 680M (RGBA f32, 468 ms): the round
/// trip copies the frame to the device and back.
#[cfg(feature = "wgpu")]
const GPU_ITERATION: Duration = Duration::from_millis(470);

/// `atomic/blend/cpu/<variant>/<mode>/<format>`: one row pair of the standard frame in every mode,
/// through the scalar reference and the format's kernel on each tier this CPU has. Per pixel,
/// reading both rows.
pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "blend", Scale::Micros);
    for format in ColorFormat::ALL {
        let mut layers = None;
        let mut out =
            InterleavedImage::new_zeroed(InterleavedDesc::new(USize2::new(FRAME.width, 1), format));
        group.throughput(fixture::throughput(FRAME.width, 2 * format.byte_len()));
        let kernel = BlendKernel::of(format);
        for mode in BlendMode::iter() {
            let params = Blend::new(mode, ALPHA);
            for variant in Variant::all(Some(kernel)) {
                let input = format!(
                    "{}/{}",
                    fixture::variant_id(mode),
                    fixture::format_id(format)
                );
                group.bench_function(
                    BenchmarkId::new(Backend::Cpu.case(variant.id()), input),
                    |b| {
                        let [src, dst] = layers.get_or_insert_with(|| {
                            [1, 2].map(|seed| scene::sky(format, USize2::new(FRAME.width, 1), seed))
                        });
                        let (src, dst, out) = (src.bytes(), dst.bytes(), out.bytes_mut());
                        if let Variant::Kernel { kernel, tier } = variant {
                            b.iter(|| {
                                kernel.blend_row(
                                    tier,
                                    params,
                                    black_box(src),
                                    black_box(dst),
                                    black_box(&mut *out),
                                );
                            });
                        } else {
                            b.iter(|| {
                                cpu::internals::scalar_row(
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
    for format in ColorFormat::ALL {
        let mut layers = None;
        let pixels = FRAME.pixel_count();
        group.throughput(fixture::throughput(pixels, 2 * format.byte_len()));
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
                        let mut output = InterleavedImage::new_zeroed(src.desc());
                        b.iter(|| params.apply_cpu(black_box(src), black_box(dst), &mut output));
                    }
                    #[cfg(feature = "wgpu")]
                    Backend::Gpu => {
                        let gpu = backends.gpu();
                        b.iter_with_large_drop(|| {
                            let top = GpuImage::from_image(gpu, src).unwrap();
                            let bottom = GpuImage::from_image(gpu, dst).unwrap();
                            let mut output = GpuImage::new_empty(gpu, src.desc()).unwrap();
                            params.apply_gpu(gpu, &top, &bottom, &mut output);
                            output.to_image(gpu).unwrap()
                        });
                    }
                }
            });
        }
    }
    group.finish();
}
