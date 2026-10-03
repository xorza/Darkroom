//! Interpolation benchmarks for optimization tracking.

use crate::testing::prelude::*;
use crate::testing::synthetic::patterns;
use std::hint::black_box;

use ::quickbench::quick_bench;

use crate::stacking::registration::config::{self, InterpolationMethod};
use crate::stacking::registration::resample::internals::warp_plane;
use crate::stacking::registration::resample::kernel::LanczosOrder;
use crate::stacking::registration::resample::row_positions::RowPositions;
use crate::stacking::registration::resample::{self, quality, row};
use crate::stacking::registration::transform::{Transform, WarpTransform};

/// One plane of a `side`-square gradient warped by the test transform with `method`.
fn bench_plane_warp(b: quickbench::Bencher, side: usize, method: InterpolationMethod) {
    let input = patterns::diagonal_gradient(Size2us::new(side, side));
    let mut output = Buffer2::new_default(side, side);
    let transform = create_test_transform();
    let params = config::internals::warp_params(method);

    b.bench(|| {
        warp_plane(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
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
    let input = patterns::diagonal_gradient(Size2us::new(1024, 1024));
    let mut output = Buffer2::new_default(1024, 1024);
    let transform = create_test_transform();
    let wt = WarpTransform::new(transform);
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);

    let size = Size2us::new(input.width(), input.height());
    let mut positions = RowPositions::default();
    b.bench(|| {
        for (y, output_row) in black_box(&mut output)
            .pixels_mut()
            .chunks_mut(size.width)
            .enumerate()
        {
            positions.fill(y, size.width, &wt, size);
            row::sample_row(
                black_box(&input),
                positions.positions(),
                params.method,
                params.border_value,
                output_row,
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
    let pixels = patterns::diagonal_gradient(size).pixels().to_vec();
    let image =
        LinearImage::from_pixels(ImageDimensions::new((size.width, size.height), 1), pixels);
    let transform = create_test_transform();
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);

    b.bench(|| {
        resample::warp(
            black_box(&image),
            &black_box(WarpTransform::new(transform)),
            params,
        )
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_lut_lookup(b: quickbench::Bencher) {
    let lut = LanczosOrder::Three.lut();
    let test_values: Vec<f32> = (0..1000).map(|i| (i as f32 / 1000.0) * 3.0 - 1.5).collect();

    b.bench(|| {
        let mut sum = 0.0f32;
        for &x in black_box(&test_values) {
            sum += lut.lookup(x);
        }
        black_box(sum)
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
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);
    b.bench(|| {
        let mut buffers = resample::WarpBuffers::new(image.dimensions());
        buffers.warp_into(
            black_box(&image),
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
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);
    let mut buffers = resample::WarpBuffers::new(image.dimensions());
    b.bench(|| {
        buffers.warp_into(
            black_box(&image),
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
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);
    let mut buffers = resample::WarpBuffers::new(image.dimensions());
    b.bench(|| buffers.warp_into(black_box(&image), black_box(&warp), params));
}

/// A 2k RGB frame through `warp_into` with a homography: numerators and denominator are affine in x,
/// so a row can step them.
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
    let params = config::internals::warp_params(InterpolationMethod::Lanczos3);
    let mut buffers = resample::WarpBuffers::new(image.dimensions());
    b.bench(|| buffers.warp_into(black_box(&image), black_box(&warp), params));
}

/// The test transform with an order-3 SIP fitted to a mild radial field over `size`.
fn sip_warp(size: Size2us) -> WarpTransform {
    use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};
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
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..SipConfig::default()
    };
    let fit = SipPolynomial::fit_from_transform(&reference, &target, &transform, &config).unwrap();
    WarpTransform::with_sip(transform, fit.polynomial)
}
