//! The transform cases, a rotation by π/6 about the centre over all nine formats and both
//! filters: each kernel on one tile, and the op on the standard frame on each backend.

use std::f32::consts::PI;
use std::hint::black_box;
#[cfg(feature = "wgpu")]
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};
use glam::Vec2;

use common::Size2us;

use crate::bench::fixture::{self, Backend, Backends, Kind, Scale, Variant};
use crate::bench::scene::{self, FRAME, TILE};
use crate::color::color_format::{ALL_FORMATS, ColorFormat};
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;
#[cfg(feature = "wgpu")]
use crate::ops::transform::pipeline::GpuTransformPipeline;
use crate::ops::transform::{FilterMode, Transform, cpu};

/// The slowest GPU case's iteration as measured on a Radeon 680M (bilinear RGBA f32, 326 ms):
/// the round trip copies the frame to the device and back.
#[cfg(feature = "wgpu")]
const GPU_ITERATION: Duration = Duration::from_millis(330);

const FILTERS: [FilterMode; 2] = [FilterMode::Nearest, FilterMode::Bilinear];

/// `atomic/transform/cpu/<variant>/<filter>/<format>`: one 512 × 512 tile through the scalar
/// reference and each tier's kernel. The kernel takes the image and fans its rows out on rayon,
/// as dispatch hands it. Per output pixel, reading the tile.
pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "transform", Scale::Frame);
    for format in ALL_FORMATS {
        let mut tile = None;
        let mut output =
            Image::new_black(ImageDesc::new(Size2us::new(TILE, TILE), format)).unwrap();
        group.throughput(fixture::throughput(TILE * TILE, format.byte_count()));
        for filter in FILTERS {
            let transform = rotation(Size2us::new(TILE, TILE), filter);
            for variant in Variant::all(|tier| cpu::packed_kernel(tier, format, filter)) {
                let id = BenchmarkId::new(Backend::Cpu.case(variant.name), case_id(filter, format));
                group.bench_function(id, |b| {
                    let tile =
                        tile.get_or_insert_with(|| scene::sky(format, Size2us::new(TILE, TILE), 1));
                    if let Some(kernel) = variant.kernel {
                        b.iter(|| unsafe {
                            kernel(&transform, black_box(tile), black_box(&mut output));
                        });
                    } else {
                        b.iter(|| {
                            cpu::apply_scalar(&transform, black_box(tile), black_box(&mut output));
                        });
                    }
                });
            }
        }
    }
    group.finish();
}

/// `integration/transform/<backend>/<filter>/<format>`: the op on the standard frame. On the CPU
/// into a caller's output; on the GPU the upload, the op and the readback. Per output pixel,
/// reading the frame.
pub(crate) fn integration(c: &mut Criterion, backends: &Backends) {
    let mut group = Kind::Integration.group(c, "transform", Scale::Frame);
    #[cfg(feature = "wgpu")]
    let mut pipeline = None;
    for format in ALL_FORMATS {
        let mut source = None;
        let pixels = FRAME.pixel_count();
        group.throughput(fixture::throughput(pixels, format.byte_count()));
        for filter in FILTERS {
            let transform = rotation(FRAME, filter);
            for backend in backends.all() {
                match backend {
                    Backend::Cpu => Scale::Frame,
                    #[cfg(feature = "wgpu")]
                    Backend::Gpu => Scale::Set {
                        iteration: GPU_ITERATION,
                    },
                }
                .apply(&mut group);
                let id = BenchmarkId::new(backend.id(), case_id(filter, format));
                group.bench_function(id, |b| {
                    let frame = source.get_or_insert_with(|| scene::sky(format, FRAME, 1));
                    match backend {
                        Backend::Cpu => {
                            let mut output = Image::new_black(frame.desc()).unwrap();
                            b.iter(|| transform.apply_cpu(black_box(frame), &mut output));
                        }
                        #[cfg(feature = "wgpu")]
                        Backend::Gpu => {
                            let gpu = backends.gpu();
                            let pipeline =
                                pipeline.get_or_insert_with(|| GpuTransformPipeline::new(gpu));
                            b.iter_with_large_drop(|| {
                                let input = GpuImage::from_image(gpu, frame).unwrap();
                                let mut output = GpuImage::new_empty(gpu, frame.desc()).unwrap();
                                transform.apply_gpu(gpu, pipeline, &input, &mut output);
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
fn rotation(size: Size2us, filter: FilterMode) -> Transform {
    let centre = Vec2::new(size.width as f32 / 2.0, size.height as f32 / 2.0);
    Transform::new()
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
