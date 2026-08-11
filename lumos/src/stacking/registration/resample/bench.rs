//! Interpolation benchmarks for optimization tracking.

use crate::testing::prelude::*;
use std::hint::black_box;

use ::quickbench::quick_bench;

use crate::stacking::registration::config::{self, InterpolationMethod};
use crate::stacking::registration::resample::kernel::internals as kernel_test_support;
use crate::stacking::registration::resample::{self, kernel, plane, quality, row};
use crate::stacking::registration::transform::{Transform, WarpTransform};

/// Create a test image of specified size filled with gradient pattern.
fn create_test_image(size: Size2us) -> Buffer2<f32> {
    let mut data = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            // Gradient pattern with some variation
            data[size.index_of(Vec2us::new(x, y))] = ((x + y) % 256) as f32 / 255.0;
        }
    }
    Buffer2::new(size.width, size.height, data)
}

/// Create a small rotation transform for realistic warping.
fn create_test_transform() -> Transform {
    // Small rotation (0.5 degrees) + small translation
    let angle = 0.5_f64.to_radians();
    Transform::similarity(DVec2::new(5.0, 3.0), -angle, 1.0)
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos3_1k(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(1024, 1024));
    let mut output = Buffer2::new_default(1024, 1024);
    let transform = create_test_transform();

    b.bench(|| {
        plane::warp(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &config::internals::warp_params(InterpolationMethod::Lanczos3),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos3_2k(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(2048, 2048));
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        plane::warp(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &config::internals::warp_params(InterpolationMethod::Lanczos3),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos3_4k(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(4096, 4096));
    let mut output = Buffer2::new_default(4096, 4096);
    let transform = create_test_transform();

    b.bench(|| {
        plane::warp(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &config::internals::warp_params(InterpolationMethod::Lanczos3),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_bilinear_2k(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(2048, 2048));
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        plane::warp(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &config::internals::warp_params(InterpolationMethod::Bilinear),
        );
    });
}

/// Single-threaded 1k warp to measure per-thread throughput without rayon overhead.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos3_1k_single_thread(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(1024, 1024));
    let mut output = Buffer2::new_default(1024, 1024);
    let transform = create_test_transform();
    let wt = WarpTransform::new(transform);
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);

    b.bench(|| {
        let width = input.width();
        for (y, row) in black_box(&mut output)
            .pixels_mut()
            .chunks_mut(width)
            .enumerate()
        {
            row::lanczos(black_box(&input), row, y, &wt, &params);
        }
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_bicubic_2k(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(2048, 2048));
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        plane::warp(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &config::internals::warp_params(InterpolationMethod::Bicubic),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos4_2k(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(2048, 2048));
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        plane::warp(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &config::internals::warp_params(InterpolationMethod::Lanczos4),
        );
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos2_2k(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(2048, 2048));
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        plane::warp(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &config::internals::warp_params(InterpolationMethod::Lanczos2),
        );
    });
}

/// The quality maps against the plane warp beside them, at the same size and method.
///
/// `warp` pays this once per frame and the plane warp once per channel, so the ratio between these
/// two is what decides how much of a registered frame's warp time is spent on the quality planes —
/// see `bench_warp_with_quality_lanczos3_1k` for the combined figure a mono frame actually pays.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_quality_maps_lanczos3_1k(b: quickbench::Bencher) {
    let transform = create_test_transform();

    b.bench(|| {
        quality::internals::maps(
            black_box(Size2us::new(1024, 1024)),
            &black_box(WarpTransform::new(transform)),
            InterpolationMethod::Lanczos3,
        )
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_quality_maps_bilinear_1k(b: quickbench::Bencher) {
    let transform = create_test_transform();

    b.bench(|| {
        quality::internals::maps(
            black_box(Size2us::new(1024, 1024)),
            &black_box(WarpTransform::new(transform)),
            InterpolationMethod::Bilinear,
        )
    });
}

/// One whole frame through the public entry point: the plane warp plus the quality maps, which is
/// what the pipeline pays per registered frame. Single-channel, so the maps are charged against one
/// plane warp rather than three.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_with_quality_lanczos3_1k(b: quickbench::Bencher) {
    let size = Size2us::new(1024, 1024);
    let pixels = create_test_image(size).pixels().to_vec();
    let image =
        LinearImage::from_pixels(ImageDimensions::new((size.width, size.height), 1), pixels);
    let transform = create_test_transform();
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);

    b.bench(|| {
        resample::warp(
            black_box(&image),
            &black_box(WarpTransform::new(transform)),
            &params,
        )
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_lut_lookup(b: quickbench::Bencher) {
    let lut = kernel::get_lanczos_lut(3);
    let test_values: Vec<f32> = (0..1000).map(|i| (i as f32 / 1000.0) * 3.0 - 1.5).collect();

    b.bench(|| {
        let mut sum = 0.0f32;
        for &x in black_box(&test_values) {
            sum += lut.lookup(x);
        }
        black_box(sum)
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_interpolate_lanczos3_single(b: quickbench::Bencher) {
    let input = create_test_image(Size2us::new(256, 256));
    // Test positions near center
    let positions: Vec<(f32, f32)> = (0..1000)
        .map(|i| {
            let x = 50.0 + (i as f32 / 10.0);
            let y = 50.0 + (i as f32 / 15.0);
            (x, y)
        })
        .collect();

    b.bench(|| {
        let mut sum = 0.0f32;
        for &(x, y) in black_box(&positions) {
            sum += kernel_test_support::interpolate_lanczos(&input, Vec2::new(x, y), 3, 0.0);
        }
        black_box(sum)
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
/// One frame's warp into freshly allocated planes against the same warp into planes a previous
/// frame left behind — the spill tier's per-frame cost with and without the reuse
/// `try_par_map_bounded_owned`'s slot gives it.
///
/// The gap is first-touch page faults: `WarpBuffers::new` hands back lazily-mapped zero pages and
/// every one of them faults as the warp writes it. Sized at 16 MP because that is where the effect
/// is legible — at 1 MP the three planes are 12 MiB and the difference sits inside the noise.
fn bench_warp_into_fresh_4k(b: quickbench::Bencher) {
    let size = Size2us::new(4096, 4096);
    let pixels = create_test_image(size).pixels().to_vec();
    let image =
        LinearImage::from_pixels(ImageDimensions::new((size.width, size.height), 1), pixels);
    let transform = create_test_transform();
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);
    b.bench(|| {
        let mut buffers = resample::WarpBuffers::new(image.dimensions());
        buffers.warp_into(
            black_box(&image),
            &black_box(WarpTransform::new(transform)),
            &params,
        );
        black_box(buffers)
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_into_reused_4k(b: quickbench::Bencher) {
    let size = Size2us::new(4096, 4096);
    let pixels = create_test_image(size).pixels().to_vec();
    let image =
        LinearImage::from_pixels(ImageDimensions::new((size.width, size.height), 1), pixels);
    let transform = create_test_transform();
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);
    let mut buffers = resample::WarpBuffers::new(image.dimensions());
    b.bench(|| {
        buffers.warp_into(
            black_box(&image),
            &black_box(WarpTransform::new(transform)),
            &params,
        );
    });
}
