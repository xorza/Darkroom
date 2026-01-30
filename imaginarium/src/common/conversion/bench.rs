//! Benchmarks for row conversion operations (SIMD vs Scalar).

use super::conversion_scalar::{ConversionInfo, dispatch_convert_row_scalar};
use super::conversion_simd::get_simd_row_converter;
use crate::common::color_format::ColorFormat;
use bench::quick_bench;
use std::hint::black_box;

const WIDTH_4K: usize = 4096;

// Helper to create test row data
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

// ============ RGBA_U8 -> RGB_U8 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgba_u8_to_rgb_u8(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 4);
    let mut dst = vec![0u8; WIDTH_4K * 3];

    let from_fmt = ColorFormat::RGBA_U8;
    let to_fmt = ColorFormat::RGB_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ RGB_U8 -> RGBA_U8 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgb_u8_to_rgba_u8(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 3);
    let mut dst = vec![0u8; WIDTH_4K * 4];

    let from_fmt = ColorFormat::RGB_U8;
    let to_fmt = ColorFormat::RGBA_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ RGBA_U8 -> L_U8 (luminance) ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgba_u8_to_l_u8(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 4);
    let mut dst = vec![0u8; WIDTH_4K];

    let from_fmt = ColorFormat::RGBA_U8;
    let to_fmt = ColorFormat::L_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ RGB_U8 -> L_U8 (luminance) ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgb_u8_to_l_u8(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 3);
    let mut dst = vec![0u8; WIDTH_4K];

    let from_fmt = ColorFormat::RGB_U8;
    let to_fmt = ColorFormat::L_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ L_U8 -> RGBA_U8 (expansion) ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_l_u8_to_rgba_u8(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 1);
    let mut dst = vec![0u8; WIDTH_4K * 4];

    let from_fmt = ColorFormat::L_U8;
    let to_fmt = ColorFormat::RGBA_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ L_U8 -> RGB_U8 (expansion) ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_l_u8_to_rgb_u8(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 1);
    let mut dst = vec![0u8; WIDTH_4K * 3];

    let from_fmt = ColorFormat::L_U8;
    let to_fmt = ColorFormat::RGB_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ LA_U8 -> RGBA_U8 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_la_u8_to_rgba_u8(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 2);
    let mut dst = vec![0u8; WIDTH_4K * 4];

    let from_fmt = ColorFormat::LA_U8;
    let to_fmt = ColorFormat::RGBA_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ RGBA_U8 -> LA_U8 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgba_u8_to_la_u8(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 4);
    let mut dst = vec![0u8; WIDTH_4K * 2];

    let from_fmt = ColorFormat::RGBA_U8;
    let to_fmt = ColorFormat::LA_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ RGBA_U8 -> RGBA_U16 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgba_u8_to_rgba_u16(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 4);
    let mut dst = vec![0u8; WIDTH_4K * 4 * 2];

    let from_fmt = ColorFormat::RGBA_U8;
    let to_fmt = ColorFormat::RGBA_U16;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ RGBA_U16 -> RGBA_U8 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgba_u16_to_rgba_u8(b: bench::Bencher) {
    let src = create_u16_row(WIDTH_4K, 4);
    let mut dst = vec![0u8; WIDTH_4K * 4];

    let from_fmt = ColorFormat::RGBA_U16;
    let to_fmt = ColorFormat::RGBA_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ L_U16 -> L_F32 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_l_u16_to_l_f32(b: bench::Bencher) {
    let src = create_u16_row(WIDTH_4K, 1);
    let mut dst = vec![0u8; WIDTH_4K * 4];

    let from_fmt = ColorFormat::L_U16;
    let to_fmt = ColorFormat::L_F32;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ L_F32 -> L_U16 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_l_f32_to_l_u16(b: bench::Bencher) {
    let src = create_f32_row(WIDTH_4K, 1);
    let mut dst = vec![0u8; WIDTH_4K * 2];

    let from_fmt = ColorFormat::L_F32;
    let to_fmt = ColorFormat::L_U16;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ RGBA_F32 -> RGBA_U8 ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgba_f32_to_rgba_u8(b: bench::Bencher) {
    let src = create_f32_row(WIDTH_4K, 4);
    let mut dst = vec![0u8; WIDTH_4K * 4];

    let from_fmt = ColorFormat::RGBA_F32;
    let to_fmt = ColorFormat::RGBA_U8;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    if let Some(simd_fn) = get_simd_row_converter(from_fmt, to_fmt) {
        b.bench_labeled("simd", || {
            simd_fn(black_box(&src), black_box(&mut dst), black_box(WIDTH_4K));
        });
    }

    b.bench_labeled("scalar", || {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

// ============ Scalar-only paths (no SIMD available) ============

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgba_u8_to_rgba_f32_scalar(b: bench::Bencher) {
    let src = create_u8_row(WIDTH_4K, 4);
    let mut dst = vec![0u8; WIDTH_4K * 4 * 4];

    let from_fmt = ColorFormat::RGBA_U8;
    let to_fmt = ColorFormat::RGBA_F32;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    b.bench(|| {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_rgb_u16_to_rgb_f32_scalar(b: bench::Bencher) {
    let src = create_u16_row(WIDTH_4K, 3);
    let mut dst = vec![0u8; WIDTH_4K * 3 * 4];

    let from_fmt = ColorFormat::RGB_U16;
    let to_fmt = ColorFormat::RGB_F32;
    let info = ConversionInfo::new(from_fmt, to_fmt);

    b.bench(|| {
        dispatch_convert_row_scalar(
            black_box(&src),
            black_box(&mut dst),
            black_box(WIDTH_4K),
            black_box(&info),
        );
    });
}
