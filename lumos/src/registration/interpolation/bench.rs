//! Benchmark module for interpolation.
//! Run with: cargo bench -p lumos --features bench --bench registration_interpolation

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::simd::{
    warp_image_bilinear_simd, warp_image_lanczos3_simd, warp_row_bilinear_scalar,
    warp_row_bilinear_simd, warp_row_lanczos3_scalar, warp_row_lanczos3_simd,
};
use super::{InterpolationMethod, WarpConfig, interpolate_pixel, resample_image, warp_image};
use crate::registration::types::TransformMatrix;

/// Register interpolation benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {
    benchmark_interpolation_methods(c);
    benchmark_warp_sizes(c);
    benchmark_resample(c);
    benchmark_simd_vs_scalar(c);
}

/// Generate a gradient test image.
fn generate_gradient_image(width: usize, height: usize) -> Vec<f32> {
    let mut image = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            image[y * width + x] = (x as f32 + y as f32) / (width + height) as f32;
        }
    }
    image
}

/// Benchmark different interpolation methods.
fn benchmark_interpolation_methods(c: &mut Criterion) {
    let mut group = c.benchmark_group("interpolation_methods");

    let size = 512;
    let image = generate_gradient_image(size, size);

    let methods = [
        ("nearest", InterpolationMethod::Nearest),
        ("bilinear", InterpolationMethod::Bilinear),
        ("bicubic", InterpolationMethod::Bicubic),
        ("lanczos2", InterpolationMethod::Lanczos2),
        ("lanczos3", InterpolationMethod::Lanczos3),
        ("lanczos4", InterpolationMethod::Lanczos4),
    ];

    // Benchmark single pixel interpolation
    for (name, method) in &methods {
        let config = WarpConfig {
            method: *method,
            ..Default::default()
        };

        group.bench_function(BenchmarkId::new("single_pixel", *name), |b| {
            b.iter(|| {
                let result = interpolate_pixel(
                    black_box(&image),
                    size,
                    size,
                    black_box(256.3),
                    black_box(128.7),
                    black_box(&config),
                );
                black_box(result)
            })
        });
    }

    // Benchmark row of pixels (1000 samples)
    let sample_count = 1000;
    let samples: Vec<(f32, f32)> = (0..sample_count)
        .map(|i| {
            let t = i as f32 / sample_count as f32;
            (t * (size - 1) as f32, t * (size - 1) as f32)
        })
        .collect();

    for (name, method) in &methods {
        let config = WarpConfig {
            method: *method,
            ..Default::default()
        };

        group.throughput(Throughput::Elements(sample_count as u64));

        group.bench_function(BenchmarkId::new("batch_1000", *name), |b| {
            b.iter(|| {
                let results: Vec<f32> = samples
                    .iter()
                    .map(|&(x, y)| {
                        interpolate_pixel(black_box(&image), size, size, x, y, black_box(&config))
                    })
                    .collect();
                black_box(results)
            })
        });
    }

    group.finish();
}

/// Benchmark SIMD vs Scalar implementations for bilinear and Lanczos3.
fn benchmark_simd_vs_scalar(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_vs_scalar");

    let transform = TransformMatrix::similarity(10.0, -5.0, 0.05, 1.02);
    let inverse = transform.inverse();

    // Test row warping for different sizes
    for size in [256, 512, 1024] {
        let image = generate_gradient_image(size, size);
        let y = size / 2;

        group.throughput(Throughput::Elements(size as u64));

        // Bilinear SIMD vs Scalar
        group.bench_function(
            BenchmarkId::new("bilinear_simd_row", format!("{}px", size)),
            |b| {
                b.iter(|| {
                    let mut output = vec![0.0f32; size];
                    warp_row_bilinear_simd(
                        black_box(&image),
                        size,
                        size,
                        &mut output,
                        y,
                        black_box(&inverse),
                        0.0,
                    );
                    black_box(output)
                })
            },
        );

        group.bench_function(
            BenchmarkId::new("bilinear_scalar_row", format!("{}px", size)),
            |b| {
                b.iter(|| {
                    let mut output = vec![0.0f32; size];
                    warp_row_bilinear_scalar(
                        black_box(&image),
                        size,
                        size,
                        &mut output,
                        y,
                        black_box(&inverse),
                        0.0,
                    );
                    black_box(output)
                })
            },
        );

        // Lanczos3 SIMD vs Scalar
        group.bench_function(
            BenchmarkId::new("lanczos3_simd_row", format!("{}px", size)),
            |b| {
                b.iter(|| {
                    let mut output = vec![0.0f32; size];
                    warp_row_lanczos3_simd(
                        black_box(&image),
                        size,
                        size,
                        &mut output,
                        y,
                        black_box(&inverse),
                        0.0,
                        true,
                        false,
                    );
                    black_box(output)
                })
            },
        );

        group.bench_function(
            BenchmarkId::new("lanczos3_scalar_row", format!("{}px", size)),
            |b| {
                b.iter(|| {
                    let mut output = vec![0.0f32; size];
                    warp_row_lanczos3_scalar(
                        black_box(&image),
                        size,
                        size,
                        &mut output,
                        y,
                        black_box(&inverse),
                        0.0,
                        true,
                        false,
                    );
                    black_box(output)
                })
            },
        );
    }

    // Full image warping comparison (512x512)
    let size = 512;
    let image = generate_gradient_image(size, size);

    group.throughput(Throughput::Elements((size * size) as u64));

    group.bench_function("bilinear_simd_image_512", |b| {
        b.iter(|| {
            let result = warp_image_bilinear_simd(
                black_box(&image),
                size,
                size,
                size,
                size,
                black_box(&transform),
                0.0,
            );
            black_box(result)
        })
    });

    group.bench_function("lanczos3_simd_image_512", |b| {
        b.iter(|| {
            let result = warp_image_lanczos3_simd(
                black_box(&image),
                size,
                size,
                size,
                size,
                black_box(&transform),
                0.0,
                true,
                false,
            );
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark image warping with different sizes.
fn benchmark_warp_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("warp_image_sizes");

    let transform = TransformMatrix::similarity(10.0, -5.0, 0.05, 1.02);

    for size in [128, 256, 512, 1024] {
        let image = generate_gradient_image(size, size);

        // Test with bilinear (fast)
        let bilinear_config = WarpConfig {
            method: InterpolationMethod::Bilinear,
            ..Default::default()
        };

        group.throughput(Throughput::Elements((size * size) as u64));

        group.bench_function(
            BenchmarkId::new("bilinear", format!("{}x{}", size, size)),
            |b| {
                b.iter(|| {
                    let result = warp_image(
                        black_box(&image),
                        size,
                        size,
                        size,
                        size,
                        black_box(&transform),
                        black_box(&bilinear_config),
                    );
                    black_box(result)
                })
            },
        );

        // Test with Lanczos3 (high quality)
        let lanczos_config = WarpConfig {
            method: InterpolationMethod::Lanczos3,
            ..Default::default()
        };

        group.bench_function(
            BenchmarkId::new("lanczos3", format!("{}x{}", size, size)),
            |b| {
                b.iter(|| {
                    let result = warp_image(
                        black_box(&image),
                        size,
                        size,
                        size,
                        size,
                        black_box(&transform),
                        black_box(&lanczos_config),
                    );
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark image resampling (up/downscale).
fn benchmark_resample(c: &mut Criterion) {
    let mut group = c.benchmark_group("resample");

    let src_size = 512;
    let image = generate_gradient_image(src_size, src_size);

    // Upscale 2x
    group.throughput(Throughput::Elements((1024 * 1024) as u64));
    group.bench_function("upscale_2x_bilinear", |b| {
        b.iter(|| {
            let result = resample_image(
                black_box(&image),
                src_size,
                src_size,
                1024,
                1024,
                InterpolationMethod::Bilinear,
            );
            black_box(result)
        })
    });

    group.bench_function("upscale_2x_lanczos3", |b| {
        b.iter(|| {
            let result = resample_image(
                black_box(&image),
                src_size,
                src_size,
                1024,
                1024,
                InterpolationMethod::Lanczos3,
            );
            black_box(result)
        })
    });

    // Downscale 2x
    group.throughput(Throughput::Elements((256 * 256) as u64));
    group.bench_function("downscale_2x_bilinear", |b| {
        b.iter(|| {
            let result = resample_image(
                black_box(&image),
                src_size,
                src_size,
                256,
                256,
                InterpolationMethod::Bilinear,
            );
            black_box(result)
        })
    });

    group.bench_function("downscale_2x_lanczos3", |b| {
        b.iter(|| {
            let result = resample_image(
                black_box(&image),
                src_size,
                src_size,
                256,
                256,
                InterpolationMethod::Lanczos3,
            );
            black_box(result)
        })
    });

    group.finish();
}
