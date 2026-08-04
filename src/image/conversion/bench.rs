//! Benchmarks for row conversion operations (SIMD vs Scalar).

use super::scalar::{ConversionInfo, dispatch_convert_row_scalar};
use super::simd::get_simd_row_converter;
use crate::common::color_format::{ChannelSize, ColorFormat};
use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;
use std::time::Duration;

const WIDTH_4K: usize = 4096;

fn create_u8_row(width: usize, channels: usize) -> Vec<u8> {
    (0..width * channels).map(|i| (i % 256) as u8).collect()
}

fn create_u16_row(width: usize, channels: usize) -> Vec<u8> {
    let data: Vec<u16> = (0..width * channels)
        .map(|i| ((i % 65536) * 257) as u16)
        .collect();
    bytemuck::cast_slice(&data).to_vec()
}

fn create_f32_row(width: usize, channels: usize) -> Vec<u8> {
    let data: Vec<f32> = (0..width * channels)
        .map(|i| (i % 256) as f32 / 255.0)
        .collect();
    bytemuck::cast_slice(&data).to_vec()
}

fn create_row_for_format(width: usize, format: ColorFormat) -> Vec<u8> {
    let channels = format.channel_count as usize;
    match format.channel_size {
        ChannelSize::_8bit => create_u8_row(width, channels),
        ChannelSize::_16bit => create_u16_row(width, channels),
        ChannelSize::_32bit => create_f32_row(width, channels),
    }
}

/// All conversion pairs to benchmark: (from_format, to_format)
const CONVERSION_PAIRS: &[(ColorFormat, ColorFormat)] = &[
    // Channel layout conversions (U8)
    (ColorFormat::RGBA_U8, ColorFormat::RGB_U8),
    (ColorFormat::RGB_U8, ColorFormat::RGBA_U8),
    (ColorFormat::RGBA_U8, ColorFormat::L_U8),
    (ColorFormat::RGB_U8, ColorFormat::L_U8),
    (ColorFormat::L_U8, ColorFormat::RGBA_U8),
    (ColorFormat::L_U8, ColorFormat::RGB_U8),
    // U8 <-> U16
    (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16),
    (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8),
    (ColorFormat::RGB_U8, ColorFormat::RGB_U16),
    (ColorFormat::RGB_U16, ColorFormat::RGB_U8),
    (ColorFormat::L_U8, ColorFormat::L_U16),
    (ColorFormat::L_U16, ColorFormat::L_U8),
    // U8 <-> F32
    (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32),
    (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8),
    (ColorFormat::RGB_U8, ColorFormat::RGB_F32),
    (ColorFormat::RGB_F32, ColorFormat::RGB_U8),
    (ColorFormat::L_U8, ColorFormat::L_F32),
    (ColorFormat::L_F32, ColorFormat::L_U8),
    // U16 <-> F32
    (ColorFormat::RGBA_U16, ColorFormat::RGBA_F32),
    (ColorFormat::RGB_U16, ColorFormat::RGB_F32),
    (ColorFormat::L_U16, ColorFormat::L_F32),
    (ColorFormat::L_F32, ColorFormat::L_U16),
];

pub fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("conversion/row");
    group.sample_size(50);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));

    for &(from_fmt, to_fmt) in CONVERSION_PAIRS {
        let src = create_row_for_format(WIDTH_4K, from_fmt);
        let dst_size = WIDTH_4K * to_fmt.byte_count() as usize;
        let mut dst = vec![0u8; dst_size];
        let info = ConversionInfo::new(from_fmt, to_fmt);
        // Criterion turns ids into report paths, so keep them space-free.
        let label = format!("{from_fmt}_to_{to_fmt}").replace(' ', "_");

        group.throughput(Throughput::Bytes(src.len() as u64));

        if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
            group.bench_function(BenchmarkId::new("simd", &label), |b| {
                b.iter(|| simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K)));
            });
        }

        group.bench_function(BenchmarkId::new("scalar", &label), |b| {
            b.iter(|| {
                dispatch_convert_row_scalar(
                    black_box(&src),
                    black_box(&mut dst),
                    black_box(WIDTH_4K),
                    black_box(&info),
                );
            });
        });
    }

    group.finish();
}
