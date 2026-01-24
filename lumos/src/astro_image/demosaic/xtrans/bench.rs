//! Benchmark module for X-Trans demosaic operations.
//! Run with: cargo bench --package lumos --features bench xtrans

use super::scalar::{LinearNeighborLookup, NeighborLookup, demosaic_scalar, demosaic_simd_linear};
use super::{XTransImage, XTransPattern};
use criterion::{BenchmarkId, Criterion};
use std::hint::black_box;
use std::path::Path;

/// Raw X-Trans data loaded from a RAF file.
pub struct XTransRawData {
    pub data: Vec<f32>,
    pub raw_width: usize,
    pub raw_height: usize,
    pub width: usize,
    pub height: usize,
    pub top_margin: usize,
    pub left_margin: usize,
    pub xtrans_pattern: [[u8; 6]; 6],
}

/// Loads raw X-Trans data from a RAF file for benchmarking.
///
/// # Panics
/// Panics if the file cannot be read or is not an X-Trans sensor file.
pub fn load_xtrans_data(path: &Path) -> XTransRawData {
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

    // Verify this is an X-Trans sensor
    let filters = unsafe { (*inner).idata.filters };
    assert_eq!(
        filters, 9,
        "libraw: Expected X-Trans sensor (filters=9), got filters={}",
        filters
    );

    // Get X-Trans pattern from libraw
    // SAFETY: inner is valid and xtrans is populated for X-Trans sensors
    let xtrans_raw = unsafe { (*inner).idata.xtrans };
    let mut xtrans_pattern = [[0u8; 6]; 6];
    for (i, row) in xtrans_raw.iter().enumerate() {
        for (j, &val) in row.iter().enumerate() {
            xtrans_pattern[i][j] = val as u8;
        }
    }

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

    XTransRawData {
        data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        xtrans_pattern,
    }
}

/// Register X-Trans demosaic benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, raf_file_path: &Path) {
    let raw = load_xtrans_data(raf_file_path);

    let pattern = XTransPattern::new(raw.xtrans_pattern);
    let xtrans = XTransImage::with_margins(
        &raw.data,
        raw.raw_width,
        raw.raw_height,
        raw.width,
        raw.height,
        raw.top_margin,
        raw.left_margin,
        pattern.clone(),
    );

    // Pre-compute lookups for scalar benchmark
    let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
    let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
    let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);
    let lookups = [&red_lookup, &green_lookup, &blue_lookup];

    // Pre-compute linear lookups for SIMD benchmark
    let linear_red = LinearNeighborLookup::new(&xtrans.pattern, 0, xtrans.raw_width);
    let linear_green = LinearNeighborLookup::new(&xtrans.pattern, 1, xtrans.raw_width);
    let linear_blue = LinearNeighborLookup::new(&xtrans.pattern, 2, xtrans.raw_width);
    let linear_lookups = [&linear_red, &linear_green, &linear_blue];

    let mut group = c.benchmark_group("xtrans_demosaic");
    group.sample_size(20);

    group.bench_function(BenchmarkId::new("bilinear", "scalar"), |b| {
        b.iter(|| black_box(demosaic_scalar(&xtrans, &lookups)))
    });

    // Non-parallel SIMD benchmark for fair comparison
    group.bench_function(BenchmarkId::new("bilinear", "simd"), |b| {
        b.iter(|| black_box(demosaic_simd_linear(&xtrans, &lookups, &linear_lookups)))
    });

    group.finish();
}
