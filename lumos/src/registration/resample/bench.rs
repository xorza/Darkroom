//! Interpolation benchmarks for optimization tracking.

use crate::internals::prelude::*;
use crate::internals::synthetic::patterns;
use std::hint::black_box;

use ::quickbench::quick_bench;

use crate::registration::registration_config::{self, InterpolationMethod};
use crate::registration::resample;
use crate::registration::resample::frame_sampler::{
    FrameSampler, RowOutput, SampleMethod, WindowAxes,
};
use crate::registration::resample::row_positions::RowPositions;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::transform::{Transform, WarpTransform};

/// A mono `side`-square gradient warped by the test transform with `method`, into buffers a
/// previous frame left: the pixels and the quality maps one plane costs.
fn bench_plane_warp(b: quickbench::Bencher, side: usize, method: InterpolationMethod) {
    let size = Size2us::new(side, side);
    let image = LinearImage::from_pixels(
        ImageDimensions::new((side, side), 1),
        patterns::diagonal_gradient(size).pixels().to_vec(),
    );
    let transform = WarpTransform::new(create_test_transform());
    let params = registration_config::internals::warp_params(method);
    let mut buffers = resample::WarpBuffers::new(image.dimensions());
    b.bench(|| {
        buffers.warp_into(
            black_box(&SourceImage::of(&image)),
            black_box(&transform),
            params,
        );
    });
}

/// Create a small rotation transform for realistic warping.
fn create_test_transform() -> Transform {
    // Small rotation (0.5 degrees) + small translation
    let angle = 0.5_f64.to_radians();
    Transform::similarity(DVec2::new(5.0, 3.0), -angle, 1.0)
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos3_1k(b: quickbench::Bencher) {
    bench_plane_warp(b, 1024, InterpolationMethod::Lanczos3);
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos3_2k(b: quickbench::Bencher) {
    bench_plane_warp(b, 2048, InterpolationMethod::Lanczos3);
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos3_4k(b: quickbench::Bencher) {
    bench_plane_warp(b, 4096, InterpolationMethod::Lanczos3);
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_bilinear_2k(b: quickbench::Bencher) {
    bench_plane_warp(b, 2048, InterpolationMethod::Bilinear);
}

/// Single-threaded 1k warp to measure per-thread throughput without rayon overhead.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos3_1k_single_thread(b: quickbench::Bencher) {
    let size = Size2us::new(1024, 1024);
    let image = LinearImage::from_pixels(
        ImageDimensions::new((size.width, size.height), 1),
        patterns::diagonal_gradient(size).pixels().to_vec(),
    );
    let mut output = Buffer2::new_default(size.width, size.height);
    let mut coverage = vec![0.0; size.width];
    let mut confidence = vec![0.0; size.width];
    let transform = WarpTransform::new(create_test_transform());
    let params = registration_config::internals::warp_params(InterpolationMethod::Lanczos3);
    let method = SampleMethod::for_frame(params, &transform, size);
    let source = SourceImage::of(&image);
    let sampler = FrameSampler::new(method, &source, None, params.border_value);
    let mut positions = RowPositions::default();
    let mut axes = WindowAxes::default();
    b.bench(|| {
        for (y, output_row) in black_box(&mut output)
            .pixels_mut()
            .chunks_mut(size.width)
            .enumerate()
        {
            positions.fill(y, size.width, &transform, size);
            sampler.sample_row(
                positions.positions(),
                &mut axes,
                RowOutput {
                    channels: &mut [output_row],
                    coverage: &mut coverage,
                    confidence: &mut confidence,
                },
            );
        }
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_bicubic_2k(b: quickbench::Bencher) {
    bench_plane_warp(b, 2048, InterpolationMethod::Bicubic);
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos4_2k(b: quickbench::Bencher) {
    bench_plane_warp(b, 2048, InterpolationMethod::Lanczos4);
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_lanczos2_2k(b: quickbench::Bencher) {
    bench_plane_warp(b, 2048, InterpolationMethod::Lanczos2);
}

/// One whole frame through the public entry point: the plane warp plus the quality maps, which is
/// what the pipeline pays per registered frame. Single-channel, so the maps are charged against one
/// plane warp rather than three.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_with_quality_lanczos3_1k(b: quickbench::Bencher) {
    let size = Size2us::new(1024, 1024);
    let pixels = patterns::diagonal_gradient(size).pixels().to_vec();
    let image =
        LinearImage::from_pixels(ImageDimensions::new((size.width, size.height), 1), pixels);
    let transform = create_test_transform();
    let params = registration_config::internals::warp_params(InterpolationMethod::Lanczos3);

    b.bench(|| {
        resample::warp(
            black_box(&image),
            &black_box(WarpTransform::new(transform)),
            params,
        )
    });
}

/// One frame's warp into freshly allocated planes against the same warp into planes a previous
/// frame left behind — the spill tier's per-frame cost with and without the reuse
/// `try_par_map_bounded_owned`'s slot gives it.
///
/// The gap is first-touch page faults: `WarpBuffers::new` hands back lazily-mapped zero pages and
/// every one of them faults as the warp writes it. Sized at 16 MP because that is where the effect
/// is legible — at 1 MP the three planes are 12 MiB and the difference sits inside the noise.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_into_fresh_4k(b: quickbench::Bencher) {
    let size = Size2us::new(4096, 4096);
    let pixels = patterns::diagonal_gradient(size).pixels().to_vec();
    let image =
        LinearImage::from_pixels(ImageDimensions::new((size.width, size.height), 1), pixels);
    let transform = create_test_transform();
    let params = registration_config::internals::warp_params(InterpolationMethod::Lanczos3);
    b.bench(|| {
        let mut buffers = resample::WarpBuffers::new(image.dimensions());
        buffers.warp_into(
            black_box(&SourceImage::of(&image)),
            &black_box(WarpTransform::new(transform)),
            params,
        );
        black_box(buffers)
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_into_reused_4k(b: quickbench::Bencher) {
    let size = Size2us::new(4096, 4096);
    let pixels = patterns::diagonal_gradient(size).pixels().to_vec();
    let image =
        LinearImage::from_pixels(ImageDimensions::new((size.width, size.height), 1), pixels);
    let transform = create_test_transform();
    let params = registration_config::internals::warp_params(InterpolationMethod::Lanczos3);
    let mut buffers = resample::WarpBuffers::new(image.dimensions());
    b.bench(|| {
        buffers.warp_into(
            black_box(&SourceImage::of(&image)),
            &black_box(WarpTransform::new(transform)),
            params,
        );
    });
}

/// A 2k RGB frame through `warp_into` with a SIP correction: the transform is non-linear, so this
/// is the case where every channel, the quality maps and validity each need the source positions.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_into_rgb_sip_2k(b: quickbench::Bencher) {
    let size = Size2us::new(2048, 2048);
    let plane = patterns::diagonal_gradient(size).pixels().to_vec();
    let image = LinearImage::from_planar_channels(
        ImageDimensions::new((size.width, size.height), 3),
        [plane.clone(), plane.clone(), plane],
    );
    let warp = sip_warp(size);
    let params = registration_config::internals::warp_params(InterpolationMethod::Lanczos3);
    let mut buffers = resample::WarpBuffers::new(image.dimensions());
    b.bench(|| {
        buffers.warp_into(
            black_box(&SourceImage::of(&image)),
            black_box(&warp),
            params,
        );
    });
}

/// A 2k RGB frame through `warp_into` with a homography: numerators and denominator are affine in
/// x, so a row can step them.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_warp_into_rgb_homography_2k(b: quickbench::Bencher) {
    let size = Size2us::new(2048, 2048);
    let plane = patterns::diagonal_gradient(size).pixels().to_vec();
    let image = LinearImage::from_planar_channels(
        ImageDimensions::new((size.width, size.height), 3),
        [plane.clone(), plane.clone(), plane],
    );
    let warp = WarpTransform::new(Transform::homography([
        1.0, 0.003, 4.0, -0.002, 1.0, 2.5, 1e-7, -2e-7,
    ]));
    let params = registration_config::internals::warp_params(InterpolationMethod::Lanczos3);
    let mut buffers = resample::WarpBuffers::new(image.dimensions());
    b.bench(|| {
        buffers.warp_into(
            black_box(&SourceImage::of(&image)),
            black_box(&warp),
            params,
        );
    });
}

/// The test transform with an order-3 SIP fitted to a mild radial field over `size`.
fn sip_warp(size: Size2us) -> WarpTransform {
    use crate::registration::distortion::sip::SipPolynomial;
    let transform = create_test_transform();
    let center = DVec2::new(size.width as f64 / 2.0, size.height as f64 / 2.0);
    let mut reference = Vec::new();
    for y in (0..size.height).step_by(128) {
        for x in (0..size.width).step_by(128) {
            reference.push(DVec2::new(x as f64, y as f64));
        }
    }
    let target: Vec<DVec2> = reference
        .iter()
        .map(|&r| {
            let d = r - center;
            transform.apply(r + d * 2e-9 * d.length_squared())
        })
        .collect();
    let sip = SipPolynomial::fitted_under(&transform, &reference, &target, 3, center);
    WarpTransform::with_sip(transform, sip)
}
