//! Interpolation benchmarks for optimization tracking.

use std::hint::black_box;

use ::bench::quick_bench;

use super::*;
use crate::common::Buffer2;
use crate::math::DMat3;
use crate::registration::config::InterpolationMethod;
use crate::registration::transform::{Transform, TransformType, WarpTransform};

/// Create a test image of specified size filled with gradient pattern.
fn create_test_image(width: usize, height: usize) -> Buffer2<f32> {
    let mut data = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            // Gradient pattern with some variation
            data[y * width + x] = ((x + y) % 256) as f32 / 255.0;
        }
    }
    Buffer2::new(width, height, data)
}

/// Create a small rotation transform for realistic warping.
fn create_test_transform() -> Transform {
    // Small rotation (0.5 degrees) + small translation
    let angle = 0.5_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    Transform::from_matrix(
        DMat3::from_array([cos_a, sin_a, 5.0, -sin_a, cos_a, 3.0, 0.0, 0.0, 1.0]),
        TransformType::Similarity,
    )
}

// ============================================================================
// Single-channel warp benchmarks (what we're optimizing)
// ============================================================================

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_warp_lanczos3_1k(b: bench::Bencher) {
    let input = create_test_image(1024, 1024);
    let mut output = Buffer2::new_default(1024, 1024);
    let transform = create_test_transform();

    b.bench(|| {
        warp_image(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 }),
        );
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_warp_lanczos3_2k(b: bench::Bencher) {
    let input = create_test_image(2048, 2048);
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        warp_image(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 }),
        );
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_warp_lanczos3_4k(b: bench::Bencher) {
    let input = create_test_image(4096, 4096);
    let mut output = Buffer2::new_default(4096, 4096);
    let transform = create_test_transform();

    b.bench(|| {
        warp_image(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 }),
        );
    });
}

// ============================================================================
// Bilinear baseline for comparison
// ============================================================================

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_warp_bilinear_2k(b: bench::Bencher) {
    let input = create_test_image(2048, 2048);
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        warp_image(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &WarpParams::new(InterpolationMethod::Bilinear),
        );
    });
}

// ============================================================================
// Micro-benchmarks for specific functions
// ============================================================================

/// Single-threaded 1k warp to measure per-thread throughput without rayon overhead.
#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_warp_lanczos3_1k_single_thread(b: bench::Bencher) {
    let input = create_test_image(1024, 1024);
    let mut output = Buffer2::new_default(1024, 1024);
    let transform = create_test_transform();
    let wt = WarpTransform::new(transform);
    let params = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 });

    b.bench(|| {
        let width = input.width();
        for (y, row) in black_box(&mut output)
            .pixels_mut()
            .chunks_mut(width)
            .enumerate()
        {
            warp::warp_row_lanczos3(black_box(&input), row, y, &wt, &params);
        }
    });
}

/// Single-threaded 1k warp WITHOUT deringing, to measure soft-clamp overhead.
#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_warp_lanczos3_1k_no_dering(b: bench::Bencher) {
    let input = create_test_image(1024, 1024);
    let mut output = Buffer2::new_default(1024, 1024);
    let transform = create_test_transform();
    let wt = WarpTransform::new(transform);
    let params = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: -1.0 });

    b.bench(|| {
        let width = input.width();
        for (y, row) in black_box(&mut output)
            .pixels_mut()
            .chunks_mut(width)
            .enumerate()
        {
            warp::warp_row_lanczos3(black_box(&input), row, y, &wt, &params);
        }
    });
}

// ============================================================================
// Bicubic and Lanczos4 benchmarks (generic warp loop)
// ============================================================================

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_warp_bicubic_2k(b: bench::Bencher) {
    let input = create_test_image(2048, 2048);
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        warp_image(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &WarpParams::new(InterpolationMethod::Bicubic),
        );
    });
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_warp_lanczos4_2k(b: bench::Bencher) {
    let input = create_test_image(2048, 2048);
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        warp_image(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &WarpParams::new(InterpolationMethod::Lanczos4 { deringing: -1.0 }),
        );
    });
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_warp_lanczos2_2k(b: bench::Bencher) {
    let input = create_test_image(2048, 2048);
    let mut output = Buffer2::new_default(2048, 2048);
    let transform = create_test_transform();

    b.bench(|| {
        warp_image(
            black_box(&input),
            black_box(&mut output),
            &black_box(WarpTransform::new(transform)),
            &WarpParams::new(InterpolationMethod::Lanczos2 { deringing: -1.0 }),
        );
    });
}

// ============================================================================
// Micro-benchmarks for specific functions
// ============================================================================

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_lut_lookup(b: bench::Bencher) {
    let lut = get_lanczos_lut(3);
    let test_values: Vec<f32> = (0..1000).map(|i| (i as f32 / 1000.0) * 3.0 - 1.5).collect();

    b.bench(|| {
        let mut sum = 0.0f32;
        for &x in black_box(&test_values) {
            sum += lut.lookup(x);
        }
        black_box(sum)
    });
}

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_interpolate_lanczos3_single(b: bench::Bencher) {
    let input = create_test_image(256, 256);
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
            sum += interpolate_lanczos(&input, x, y, 3, 0.0);
        }
        black_box(sum)
    });
}
