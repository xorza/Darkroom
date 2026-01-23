//! Benchmark module for demosaic operations.
//! Run with: cargo bench --package lumos --features bench demosaic

use super::{BayerImage, CfaPattern, scalar};
use criterion::{BenchmarkId, Criterion};
use std::hint::black_box;
use std::path::Path;

pub use super::CfaPattern as CfaPatternExport;
pub use super::demosaic_bilinear;

/// Loads raw Bayer data from a RAW file for benchmarking.
///
/// # Panics
/// Panics if the file cannot be read or processed.
pub fn load_bayer_data(
    path: &Path,
) -> (
    Vec<f32>,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    CfaPattern,
) {
    use libraw_sys as sys;
    use std::fs;
    use std::slice;

    let buf = fs::read(path).expect("Failed to read raw file");

    // SAFETY: libraw_init returns a valid pointer or null on failure.
    let inner = unsafe { sys::libraw_init(0) };
    assert!(!inner.is_null(), "libraw: Failed to initialize");

    // SAFETY: inner is valid (checked above), buf is valid for the duration of this call.
    let ret = unsafe { sys::libraw_open_buffer(inner, buf.as_ptr() as *const _, buf.len()) };
    assert!(
        ret == 0,
        "libraw: Failed to open buffer, error code: {}",
        ret
    );

    // SAFETY: inner is valid and open_buffer succeeded.
    let ret = unsafe { sys::libraw_unpack(inner) };
    assert!(ret == 0, "libraw: Failed to unpack, error code: {}", ret);

    // SAFETY: inner is valid and unpack succeeded, sizes struct is initialized.
    let raw_width = unsafe { (*inner).sizes.raw_width } as usize;
    let raw_height = unsafe { (*inner).sizes.raw_height } as usize;
    let width = unsafe { (*inner).sizes.width } as usize;
    let height = unsafe { (*inner).sizes.height } as usize;
    let top_margin = unsafe { (*inner).sizes.top_margin } as usize;
    let left_margin = unsafe { (*inner).sizes.left_margin } as usize;

    // SAFETY: inner is valid, color struct is initialized after unpack.
    let black = unsafe { (*inner).color.black } as f32;
    let maximum = unsafe { (*inner).color.maximum } as f32;
    let range = maximum - black;
    assert!(range > 0.0, "libraw: Invalid color range");

    // SAFETY: inner is valid and unpack succeeded.
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    assert!(!raw_image_ptr.is_null(), "libraw: raw_image is null");

    let pixel_count = raw_width * raw_height;

    // SAFETY: raw_image_ptr is valid (checked above), and we've validated dimensions.
    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) };

    // Normalize to 0.0-1.0 range
    let data: Vec<f32> = raw_data
        .iter()
        .map(|&v| ((v as f32) - black).max(0.0) / range)
        .collect();

    // SAFETY: inner is valid
    unsafe { sys::libraw_close(inner) };

    // Assume RGGB pattern (most common for libraw)
    let cfa = CfaPattern::Rggb;

    (
        data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        cfa,
    )
}

/// Register demosaic benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, raw_file_path: &Path) {
    let (data, raw_width, raw_height, width, height, top_margin, left_margin, cfa) =
        load_bayer_data(raw_file_path);

    let bayer = BayerImage::with_margins(
        &data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        cfa,
    );

    let mut group = c.benchmark_group("demosaic");
    group.sample_size(20);

    group.bench_function(BenchmarkId::new("bilinear", "scalar"), |b| {
        b.iter(|| black_box(scalar::demosaic_bilinear_scalar(&bayer)))
    });

    group.bench_function(BenchmarkId::new("bilinear", "optimized"), |b| {
        b.iter(|| black_box(super::demosaic_bilinear(&bayer)))
    });

    group.finish();

    // Also benchmark with different image sizes to show parallelization benefits
    let mut size_group = c.benchmark_group("demosaic_scaling");
    size_group.sample_size(10);

    for size in [64, 128, 256, 512, 1024] {
        if size <= width && size <= height {
            let small_bayer = BayerImage::with_margins(
                &data,
                raw_width,
                raw_height,
                size,
                size,
                top_margin,
                left_margin,
                cfa,
            );

            size_group.bench_function(BenchmarkId::new("size", size), |b| {
                b.iter(|| black_box(super::demosaic_bilinear(&small_bayer)))
            });
        }
    }

    size_group.finish();
}
