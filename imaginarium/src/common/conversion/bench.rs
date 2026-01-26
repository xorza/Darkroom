//! Benchmark module for image format conversion operations.
//! Run with: cargo bench -p imaginarium --features bench --bench conversion

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::common::ColorFormat;
use crate::image::{Image, ImageDesc};

/// Register conversion benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    benchmark_channel_conversion(c);
    benchmark_bit_depth_conversion(c);
    benchmark_luminance_conversion(c);
}

/// Create a test image with deterministic pattern data.
fn create_test_image(width: usize, height: usize, format: ColorFormat) -> Image {
    let desc = ImageDesc::new_packed(width, height, format);
    let mut image = Image::new_black(desc).unwrap();

    // Fill with deterministic pattern
    let bytes = image.bytes_mut();
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = (i % 256) as u8;
    }

    image
}

/// Benchmark channel count conversions (RGB <-> RGBA, L <-> RGBA).
fn benchmark_channel_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("conversion_channels");

    let sizes: [(usize, usize, &str); 3] = [
        (256, 256, "256x256"),
        (1024, 1024, "1024x1024"),
        (4096, 4096, "4096x4096"),
    ];

    // RGBA_U8 -> RGB_U8 (drop alpha)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGBA_U8);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgba_u8_to_rgb_u8", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGB_U8);
                black_box(result)
            })
        });
    }

    // RGB_U8 -> RGBA_U8 (add alpha)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGB_U8);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgb_u8_to_rgba_u8", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGBA_U8);
                black_box(result)
            })
        });
    }

    // RGBA_F32 -> RGB_F32 (drop alpha, float)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGBA_F32);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgba_f32_to_rgb_f32", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGB_F32);
                black_box(result)
            })
        });
    }

    // RGB_F32 -> RGBA_F32 (add alpha, float)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGB_F32);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgb_f32_to_rgba_f32", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGBA_F32);
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark bit depth conversions (U8 <-> U16 <-> F32).
fn benchmark_bit_depth_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("conversion_bit_depth");

    let sizes: [(usize, usize, &str); 3] = [
        (256, 256, "256x256"),
        (1024, 1024, "1024x1024"),
        (4096, 4096, "4096x4096"),
    ];

    // RGBA_U8 -> RGBA_F32 (u8 to float)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGBA_U8);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgba_u8_to_rgba_f32", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGBA_F32);
                black_box(result)
            })
        });
    }

    // RGBA_F32 -> RGBA_U8 (float to u8)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGBA_F32);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgba_f32_to_rgba_u8", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGBA_U8);
                black_box(result)
            })
        });
    }

    // RGB_U8 -> RGB_F32 (u8 to float, no alpha)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGB_U8);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgb_u8_to_rgb_f32", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGB_F32);
                black_box(result)
            })
        });
    }

    // RGB_F32 -> RGB_U8 (float to u8, no alpha)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGB_F32);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgb_f32_to_rgb_u8", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGB_U8);
                black_box(result)
            })
        });
    }

    // RGBA_U8 -> RGBA_U16 (u8 to u16)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGBA_U8);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgba_u8_to_rgba_u16", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGBA_U16);
                black_box(result)
            })
        });
    }

    // RGBA_U16 -> RGBA_U8 (u16 to u8)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGBA_U16);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgba_u16_to_rgba_u8", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGBA_U8);
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark luminance (grayscale) conversions.
fn benchmark_luminance_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("conversion_luminance");

    let sizes: [(usize, usize, &str); 3] = [
        (256, 256, "256x256"),
        (1024, 1024, "1024x1024"),
        (4096, 4096, "4096x4096"),
    ];

    // RGBA_U8 -> L_U8 (color to grayscale)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGBA_U8);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgba_u8_to_l_u8", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::L_U8);
                black_box(result)
            })
        });
    }

    // L_U8 -> RGBA_U8 (grayscale to color)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::L_U8);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("l_u8_to_rgba_u8", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGBA_U8);
                black_box(result)
            })
        });
    }

    // RGB_U8 -> L_U8 (RGB to grayscale, no alpha)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGB_U8);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgb_u8_to_l_u8", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::L_U8);
                black_box(result)
            })
        });
    }

    // RGBA_F32 -> L_F32 (color to grayscale, float)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::RGBA_F32);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("rgba_f32_to_l_f32", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::L_F32);
                black_box(result)
            })
        });
    }

    // L_F32 -> RGBA_F32 (grayscale to color, float)
    for (width, height, size_name) in sizes {
        let src = create_test_image(width, height, ColorFormat::L_F32);
        let pixel_count = (width * height) as u64;

        group.throughput(Throughput::Elements(pixel_count));
        group.bench_function(BenchmarkId::new("l_f32_to_rgba_f32", size_name), |b| {
            b.iter(|| {
                let result = black_box(&src).clone().convert(ColorFormat::RGBA_F32);
                black_box(result)
            })
        });
    }

    group.finish();
}
