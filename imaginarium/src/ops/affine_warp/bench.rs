//! The transform cases, a rotation by π/6 about the centre over all nine formats and both
//! filters: each kernel on one tile, and the op on the standard frame on each backend.

use std::f32::consts::PI;
use std::hint::black_box;
#[cfg(feature = "wgpu")]
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};
use glam::Vec2;

use common::USize2;
use common::bench::{Kind, Scale};

use crate::bench::fixture::{self, Backend, Backends, Variant};
use crate::bench::scene::{self, FRAME, TILE};
use crate::color::color_format::ColorFormat;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::ops::affine_warp::cpu::BilinearKernel;
use crate::ops::affine_warp::{AffineWarp, FilterMode, cpu};

/// The slowest GPU case's iteration as measured on a Radeon 680M (bilinear RGBA f32, 326 ms):
/// the round trip copies the frame to the device and back.
#[cfg(feature = "wgpu")]
const GPU_ITERATION: Duration = Duration::from_millis(330);

const FILTERS: [FilterMode; 2] = [FilterMode::Nearest, FilterMode::Bilinear];

/// `atomic/affine_warp/cpu/<variant>/<filter>/<format>`: one 512 × 512 tile through the scalar
/// reference and, for a warp with a kernel, that kernel on each tier this CPU has. The rows fan
/// out on rayon, as dispatch hands them. Per output pixel, reading the tile.
pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "affine_warp", Scale::Millis);
    for format in ColorFormat::ALL {
        let mut tile = None;
        let desc = InterleavedDesc::new(USize2::new(TILE, TILE), format);
        let mut output = InterleavedImage::new_zeroed(desc);
        group.throughput(fixture::throughput(TILE * TILE, format.byte_len()));
        for filter in FILTERS {
            let warp = rotation(USize2::new(TILE, TILE), filter);
            for variant in Variant::all(BilinearKernel::of(&warp, desc, desc)) {
                let id = BenchmarkId::new(Backend::Cpu.case(variant.id()), case_id(filter, format));
                group.bench_function(id, |b| {
                    let tile =
                        tile.get_or_insert_with(|| scene::sky(format, USize2::new(TILE, TILE), 1));
                    if let Variant::Kernel { tier, .. } = variant {
                        b.iter(|| {
                            cpu::apply_on(tier, &warp, black_box(tile), black_box(&mut output));
                        });
                    } else {
                        b.iter(|| {
                            cpu::apply_scalar(&warp, black_box(tile), black_box(&mut output));
                        });
                    }
                });
            }
        }
    }
    group.finish();
}

/// `integration/affine_warp/<backend>/<filter>/<format>`: the op on the standard frame. On the CPU
/// into a caller's output; on the GPU the upload, the op and the readback. Per output pixel,
/// reading the frame.
pub(crate) fn integration(c: &mut Criterion, backends: &Backends) {
    let mut group = Kind::Integration.group(c, "affine_warp", Scale::Millis);
    for format in ColorFormat::ALL {
        let mut source = None;
        let pixels = FRAME.pixel_count();
        group.throughput(fixture::throughput(pixels, format.byte_len()));
        for filter in FILTERS {
            let warp = rotation(FRAME, filter);
            for backend in backends.all() {
                match backend {
                    Backend::Cpu => Scale::Millis,
                    #[cfg(feature = "wgpu")]
                    Backend::Gpu => Scale::Slow {
                        iteration: GPU_ITERATION,
                    },
                }
                .apply(&mut group);
                let id = BenchmarkId::new(backend.id(), case_id(filter, format));
                group.bench_function(id, |b| {
                    let frame = source.get_or_insert_with(|| scene::sky(format, FRAME, 1));
                    match backend {
                        Backend::Cpu => {
                            let mut output = InterleavedImage::new_zeroed(frame.desc());
                            b.iter(|| warp.apply_cpu(black_box(frame), &mut output));
                        }
                        #[cfg(feature = "wgpu")]
                        Backend::Gpu => {
                            let gpu = backends.gpu();
                            b.iter_with_large_drop(|| {
                                let input = GpuImage::from_image(gpu, frame).unwrap();
                                let mut output = GpuImage::new_empty(gpu, frame.desc()).unwrap();
                                warp.apply_gpu(gpu, &input, &mut output);
                                output.to_image(gpu).unwrap()
                            });
                        }
                    }
                });
            }
        }
    }
    group.finish();
}

/// A rotation by π/6 about the centre of an image of `size`, sampled by `filter`.
fn rotation(size: USize2, filter: FilterMode) -> AffineWarp {
    let centre = Vec2::new(size.width as f32 / 2.0, size.height as f32 / 2.0);
    AffineWarp::new()
        .rotate_around(PI / 6.0, centre)
        .filter(filter)
}

fn case_id(filter: FilterMode, format: ColorFormat) -> String {
    format!(
        "{}/{}",
        fixture::variant_id(filter),
        fixture::format_id(format)
    )
}
