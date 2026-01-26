//! Benchmark module for image format conversion operations.
//! Run with: cargo bench -p imaginarium --features bench --bench conversion -- "<pattern>"
//!
//! Examples:
//!   cargo bench -p imaginarium --features bench --bench conversion -- "la_rgba"
//!   cargo bench -p imaginarium --features bench --bench conversion -- "u16_f32"
//!   cargo bench -p imaginarium --features bench --bench conversion -- "luminance"
//!   cargo bench -p imaginarium --features bench --bench conversion -- "channels"
//!   cargo bench -p imaginarium --features bench --bench conversion -- "bit_depth"

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::common::ColorFormat;
use crate::common::conversion::{convert_image_scalar, convert_image_simd};
use crate::image::{Image, ImageDesc};

/// Convert image using scalar implementation.
fn convert_scalar(src: &Image, to_format: ColorFormat) -> Image {
    let desc = ImageDesc::new_packed(
        src.desc().width as usize,
        src.desc().height as usize,
        to_format,
    );
    let mut dst = Image::new_black(desc).unwrap();
    convert_image_scalar(src, &mut dst).unwrap();
    dst
}

/// Convert image using SIMD implementation.
fn convert_simd(src: &Image, to_format: ColorFormat) -> Image {
    let desc = ImageDesc::new_packed(
        src.desc().width as usize,
        src.desc().height as usize,
        to_format,
    );
    let mut dst = Image::new_black(desc).unwrap();
    let _ = convert_image_simd(src, &mut dst);
    dst
}

/// Create a test image with deterministic pattern data.
fn create_test_image(format: ColorFormat) -> Image {
    let desc = ImageDesc::new_packed(4096, 4096, format);
    let mut image = Image::new_black(desc).unwrap();
    for (i, byte) in image.bytes_mut().iter_mut().enumerate() {
        *byte = (i % 256) as u8;
    }
    image
}

/// Helper to run scalar vs SIMD comparison.
fn bench(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    name: &str,
    src: &Image,
    to_format: ColorFormat,
) {
    let pixel_count = (src.desc().width * src.desc().height) as u64;
    group.throughput(Throughput::Elements(pixel_count));

    group.bench_function(BenchmarkId::new(format!("{}_scalar", name), "4k"), |b| {
        b.iter(|| black_box(convert_scalar(black_box(src), to_format)))
    });

    group.bench_function(BenchmarkId::new(format!("{}_simd", name), "4k"), |b| {
        b.iter(|| black_box(convert_simd(black_box(src), to_format)))
    });
}

pub fn benchmarks(c: &mut Criterion) {
    benchmark_la_rgba(c);
    benchmark_u16_f32(c);
    benchmark_luminance(c);
    benchmark_channels(c);
    benchmark_bit_depth(c);
}

fn benchmark_la_rgba(c: &mut Criterion) {
    let mut group = c.benchmark_group("la_rgba");
    bench(
        &mut group,
        "la_to_rgba",
        &create_test_image(ColorFormat::LA_U8),
        ColorFormat::RGBA_U8,
    );
    bench(
        &mut group,
        "rgba_to_la",
        &create_test_image(ColorFormat::RGBA_U8),
        ColorFormat::LA_U8,
    );
    group.finish();
}

fn benchmark_u16_f32(c: &mut Criterion) {
    let mut group = c.benchmark_group("u16_f32");
    bench(
        &mut group,
        "rgba_u16_to_f32",
        &create_test_image(ColorFormat::RGBA_U16),
        ColorFormat::RGBA_F32,
    );
    bench(
        &mut group,
        "rgba_f32_to_u16",
        &create_test_image(ColorFormat::RGBA_F32),
        ColorFormat::RGBA_U16,
    );
    bench(
        &mut group,
        "rgb_u16_to_f32",
        &create_test_image(ColorFormat::RGB_U16),
        ColorFormat::RGB_F32,
    );
    bench(
        &mut group,
        "rgb_f32_to_u16",
        &create_test_image(ColorFormat::RGB_F32),
        ColorFormat::RGB_U16,
    );
    bench(
        &mut group,
        "l_u16_to_f32",
        &create_test_image(ColorFormat::L_U16),
        ColorFormat::L_F32,
    );
    bench(
        &mut group,
        "l_f32_to_u16",
        &create_test_image(ColorFormat::L_F32),
        ColorFormat::L_U16,
    );
    group.finish();
}

fn benchmark_luminance(c: &mut Criterion) {
    let mut group = c.benchmark_group("luminance");
    bench(
        &mut group,
        "rgba_u8_to_l",
        &create_test_image(ColorFormat::RGBA_U8),
        ColorFormat::L_U8,
    );
    bench(
        &mut group,
        "rgb_u8_to_l",
        &create_test_image(ColorFormat::RGB_U8),
        ColorFormat::L_U8,
    );
    bench(
        &mut group,
        "l_u8_to_rgba",
        &create_test_image(ColorFormat::L_U8),
        ColorFormat::RGBA_U8,
    );
    bench(
        &mut group,
        "rgba_f32_to_l",
        &create_test_image(ColorFormat::RGBA_F32),
        ColorFormat::L_F32,
    );
    bench(
        &mut group,
        "rgb_f32_to_l",
        &create_test_image(ColorFormat::RGB_F32),
        ColorFormat::L_F32,
    );
    bench(
        &mut group,
        "l_f32_to_rgba",
        &create_test_image(ColorFormat::L_F32),
        ColorFormat::RGBA_F32,
    );
    group.finish();
}

fn benchmark_channels(c: &mut Criterion) {
    let mut group = c.benchmark_group("channels");
    bench(
        &mut group,
        "rgba_u8_to_rgb",
        &create_test_image(ColorFormat::RGBA_U8),
        ColorFormat::RGB_U8,
    );
    bench(
        &mut group,
        "rgb_u8_to_rgba",
        &create_test_image(ColorFormat::RGB_U8),
        ColorFormat::RGBA_U8,
    );
    bench(
        &mut group,
        "rgba_f32_to_rgb",
        &create_test_image(ColorFormat::RGBA_F32),
        ColorFormat::RGB_F32,
    );
    bench(
        &mut group,
        "rgb_f32_to_rgba",
        &create_test_image(ColorFormat::RGB_F32),
        ColorFormat::RGBA_F32,
    );
    group.finish();
}

fn benchmark_bit_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("bit_depth");
    bench(
        &mut group,
        "rgba_u8_to_f32",
        &create_test_image(ColorFormat::RGBA_U8),
        ColorFormat::RGBA_F32,
    );
    bench(
        &mut group,
        "rgba_f32_to_u8",
        &create_test_image(ColorFormat::RGBA_F32),
        ColorFormat::RGBA_U8,
    );
    bench(
        &mut group,
        "rgb_u8_to_f32",
        &create_test_image(ColorFormat::RGB_U8),
        ColorFormat::RGB_F32,
    );
    bench(
        &mut group,
        "rgb_f32_to_u8",
        &create_test_image(ColorFormat::RGB_F32),
        ColorFormat::RGB_U8,
    );
    bench(
        &mut group,
        "rgba_u8_to_u16",
        &create_test_image(ColorFormat::RGBA_U8),
        ColorFormat::RGBA_U16,
    );
    bench(
        &mut group,
        "rgba_u16_to_u8",
        &create_test_image(ColorFormat::RGBA_U16),
        ColorFormat::RGBA_U8,
    );
    group.finish();
}
