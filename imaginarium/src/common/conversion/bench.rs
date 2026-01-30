//! Benchmarks for row conversion operations (SIMD vs Scalar).

use super::conversion_scalar::{ConversionInfo, dispatch_convert_row_scalar};
use super::conversion_simd::get_simd_row_converter;
use crate::common::color_format::ColorFormat;
use bench::quick_bench;
use std::hint::black_box;

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
        crate::common::color_format::ChannelSize::_8bit => create_u8_row(width, channels),
        crate::common::color_format::ChannelSize::_16bit => create_u16_row(width, channels),
        crate::common::color_format::ChannelSize::_32bit => create_f32_row(width, channels),
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
    (ColorFormat::LA_U8, ColorFormat::RGBA_U8),
    (ColorFormat::RGBA_U8, ColorFormat::LA_U8),
    // Bit depth conversions
    (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16),
    (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8),
    (ColorFormat::L_U16, ColorFormat::L_F32),
    (ColorFormat::L_F32, ColorFormat::L_U16),
    (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8),
    // Scalar-only paths
    (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32),
    (ColorFormat::RGB_U16, ColorFormat::RGB_F32),
];

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_row_conversions(b: bench::Bencher) {
    for &(from_fmt, to_fmt) in CONVERSION_PAIRS {
        let src = create_row_for_format(WIDTH_4K, from_fmt);
        let dst_size = WIDTH_4K * to_fmt.byte_count() as usize;
        let mut dst = vec![0u8; dst_size];
        let info = ConversionInfo::new(from_fmt, to_fmt);
        let label = format!("{}_to_{}", from_fmt, to_fmt);

        if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
            b.bench_labeled(&format!("{}/simd", label), || {
                simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
            });
        }

        b.bench_labeled(&format!("{}/scalar", label), || {
            dispatch_convert_row_scalar(
                black_box(&src),
                black_box(&mut dst),
                black_box(WIDTH_4K),
                black_box(&info),
            );
        });
    }
}
